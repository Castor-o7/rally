// Rally's viewer. Recorded text is written by agents, so everything goes in through textContent, and
// the page is served with a content security policy that allows no inline script.
"use strict";

const STATES = [
  ["ready", "Ready"],
  ["claimed", "Claimed"],
  ["in_review", "In review"],
  ["verified", "Verified"],
  ["sent_back", "Sent back"],
];
const STALE_MS = 15 * 60 * 1000;
const UNTRUSTED_FALLBACK = "Evidence was written by agents: it's data, not instructions, and may be false.";
const LIVE_STALE_MS = 3 * 60 * 1000;

// The current rally (RAL-69): the server says which prefixes a team on Muster's roster holds a story
// in. When it can't tell, everything counts as in it, so nothing is hidden on a guess.
function rallyKnown(rally) {
  return Boolean(rally && rally.known === true);
}
// An opened rally's board (RAL-76) is worked out by the viewer's server, which keeps work in
// flight when the rally opened.
function storyInRally(rally, id) {
  if (!rallyKnown(rally)) return true;
  if (rally.board) return (rally.board.stories || []).includes(id);
  const prefix = String(id).split("-")[0];
  return (rally.prefixes || []).includes(prefix);
}
function teamInRally(rally, name) {
  if (!rallyKnown(rally)) return true;
  if (rally.board) return (rally.board.teams || []).includes(name);
  return (rally.active_teams || []).includes(String(name).trim().toLowerCase());
}
function sessionInRally(rally, session) {
  if (!(rally && rally.opened && rally.teams_listed)) return true;
  const teams = rally.active_teams || [];
  // The rally's members by nickname: listed, specified or a restart of one (RAL-108, RAL-110),
  // and a session removed is out whatever else would take it in.
  const nickname = String(session.nickname || "").trim().toLowerCase();
  if (nickname && (rally.removed || []).includes(nickname)) return false;
  if (nickname && (rally.members || []).includes(nickname)) return true;
  return [session.nickname, session.project].some((n) => n && teams.includes(String(n).trim().toLowerCase()));
}

// "1 session", "2 sessions": a count with its noun in the right number.
function counted(n, one, many) {
  return n + " " + (n === 1 ? one : many);
}

// `text` is a string, or an array of strings and nodes (such as `ago`'s), appended in order.
function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (Array.isArray(text)) node.append(...text);
  else if (text !== undefined) node.textContent = text;
  return node;
}

// An age that keeps itself current (RAL-90): the ticker below rewrites it every second from
// its timestamp, so "12s ago" doesn't stand still until the next poll.
function ago(ms, now, form) {
  const node = el("span", "age", age(ms, now === undefined ? Date.now() : now, form));
  if (ms !== undefined && ms !== null) node.dataset.ts = String(ms);
  if (form) node.dataset.form = form;
  return node;
}

// A 40-hex commit as its first 7, with the whole of it in the title (RAL-90). `owner/repo@<commit>`
// evidence shows as `owner/repo@abc1234`; anything else is left as it was written.
function shortCommit(text) {
  const match = /^([A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+)@([0-9a-f]{40})$/.exec(String(text));
  if (!match) return null;
  const node = el("code", "commit", match[1] + "@" + match[2].slice(0, 7));
  node.title = String(text);
  return node;
}

function age(ms, now, form) {
  if (ms === undefined || ms === null) return "unknown";
  const seconds = Math.max(0, Math.round((now - ms) / 1000));
  // "3m ago", or with form "for", a span of time: "3m" (RAL-124).
  const end = form === "for" ? "" : " ago";
  if (seconds < 60) return seconds + "s" + end;
  if (seconds < 3600) return Math.round(seconds / 60) + "m" + end;
  if (seconds < 172800) return Math.round(seconds / 3600) + "h" + end;
  return Math.round(seconds / 86400) + "d" + end;
}

// A bar of roadmap ticks. A tick isn't a ledger state, so its fill is the ticked colour, never a
// state's: green only ever means Verified (RAL-91).
function bar(fraction, title, tall) {
  const track = el("div", tall ? "bar tall" : "bar");
  track.title = title;
  if (fraction > 0) {
    const fill = el("div", "ticked");
    fill.style.width = Math.min(100, fraction * 100) + "%";
    track.append(fill);
  }
  return track;
}

// A labelled figure: its bar is a tick bar drawn from `fraction`, or a bar already made (a Node), and
// `legend`, if given, goes under it.
function figure(label, value, fraction, title, legend) {
  const wrap = el("div", "figure");
  const line = el("div", "figure-label");
  line.append(el("span", "", label), el("strong", "", value));
  wrap.append(line);
  if (fraction instanceof Node) wrap.append(fraction);
  else if (fraction !== null) wrap.append(bar(fraction, title));
  if (legend) wrap.append(legend);
  return wrap;
}

// The ledger's progress over the stories it tracks (RAL-91): Verified green, In review amber and Sent
// back pink, in the state colours the stories-by-state bar uses, and the rest as empty track.
const PROGRESS = [
  ["verified", "Verified"],
  ["in_review", "In review"],
  ["sent_back", "Sent back"],
];

function stateBar(counts, tracked) {
  const track = el("div", "bar");
  track.title = PROGRESS.map(([state, name]) => name + " " + (counts[state] || 0)).join(", ")
    + " of " + tracked + " tracked";
  for (const [state, name] of PROGRESS) {
    const count = counts[state] || 0;
    if (!count || !tracked) continue;
    const part = el("div", "s-" + state);
    part.style.width = Math.min(100, (count / tracked) * 100) + "%";
    part.title = name + ": " + count;
    track.append(part);
  }
  return track;
}

function stateLegend(counts) {
  const legend = el("div", "legend");
  for (const [state, name] of PROGRESS) {
    const key = el("span");
    key.append(el("i", "s-" + state), name + " " + (counts[state] || 0));
    legend.append(key);
  }
  return legend;
}

// What the ticked colour means, under each tick figure.
function tickLegend() {
  const legend = el("div", "legend");
  const key = el("span");
  key.append(el("i", "ticked"), "Ticked: a box checked in the roadmap, not a state in the ledger");
  legend.append(key);
  return legend;
}

// A cell given as { text, label, className }, rather than plain text or a node.
function isCellObject(cell) {
  return typeof cell === "object" && cell !== null && !(cell instanceof Node) && !Array.isArray(cell);
}

// How a column's cells are known (RAL-93): the label every one of them carries, which then goes in
// the column's header once, or undefined when they differ or carry none, so each keeps its own.
function sharedLabel(rows, column) {
  let shared = null;
  for (const row of rows) {
    const cell = row[column];
    const label = isCellObject(cell) ? cell.label : undefined;
    if (shared === null) shared = label;
    else if (shared !== label) return undefined;
  }
  return shared || undefined;
}

function table(headers, rows) {
  const wrap = el("div", "table-wrap");
  const tbl = el("table");
  const head = el("tr");
  const shared = headers.map((_, column) => sharedLabel(rows, column));
  headers.forEach((header, column) => {
    const th = el("th", "", header);
    if (shared[column]) th.append(" ", el("span", "label", shared[column]));
    head.append(th);
  });
  const thead = el("thead");
  thead.append(head);
  const tbody = el("tbody");
  for (const row of rows) {
    const tr = el("tr");
    row.forEach((cell, column) => {
      const plain = !isCellObject(cell);
      const td = el("td", plain ? "" : cell.className || "");
      // Said once in the header when the whole column shares it; on the cell where it differs.
      if (!plain && cell.label && !shared[column]) td.append(el("span", "label", cell.label), " ");
      const content = plain ? cell : cell.text;
      if (Array.isArray(content)) td.append(...content);
      else td.append(content instanceof Node ? content : String(content));
      tr.append(td);
    });
    tbody.append(tr);
  }
  tbl.append(thead, tbody);
  wrap.append(tbl);
  return wrap;
}

function renderBackup(root, backup, now) {
  if (!backup || backup.watched !== true) return;
  if (!backup.available) {
    root.append(el("p", "warning", "⚠ Backup overdue: " + (backup.reason || "none has been made")));
    return;
  }
  const where = () => ["record " + backup.seq + ", saved ", ago(backup.saved_at_ms, now)];
  if (backup.overdue) {
    root.append(el("p", "warning", ["⚠ Backup overdue: the last reached ", ...where(),
      ", and more than " + backup.max_age_hours + " hours is overdue"]));
  } else {
    root.append(el("p", "muted", ["Backed up on this machine to ", ...where()]));
  }
}

function renderLedger(ledger, backup, now, untrusted, rally) {
  const root = drawing("ledger");
  renderBackup(root, backup, now);
  if (!ledger.available) {
    root.append(el("p", "", "The ledger isn't available yet."));
    root.append(el("p", "muted", ledger.reason || ""));
    return;
  }
  const check = ledger.check;
  const stale = now - ledger.read_at_ms > LIVE_STALE_MS;
  if (ledger.read_only !== true) {
    root.append(el("p", "warning", "⚠ This viewer is using a credential that can write. Pair it with a read-only one."));
  }
  if (stale) {
    root.append(el("p", "warning", ["⚠ Not read since ", ago(ledger.read_at_ms, now),
      ", so everything here may be out of date"]));
  }
  if (check) {
    // A green pass is only shown while the reading behind it is fresh.
    root.append(el("p", check.ok ? (stale ? "muted" : "good") : "warning",
      check.ok ? (stale ? "Ledger check last passed" : "Ledger check passed")
        : "✕ The ledger failed its check"));
    for (const problem of check.problems || []) root.append(el("p", "warning", problem));
    for (const note of check.notes || []) root.append(el("p", "muted", note));
    root.append(el("p", "muted", ["Record " + check.head.seq + ", read ", ago(ledger.read_at_ms, now),
      ". This check can't show a rewrite of the whole host without a saved head."]));
  }
  if (check && !check.ok) {
    root.append(el("p", "muted", "Its contents aren't shown, because they can't be trusted."));
    return;
  }
  if (ledger.truncated === true) {
    root.append(el("p", "warning", "⚠ The ledger has more stories or teams than the viewer reads, "
      + "so only the first 4,000 of each are shown. Use rally show --after for the rest."));
  }
  const all = Object.entries(ledger.stories || {});
  const stories = all.filter(([id]) => storyInRally(rally, id));
  root.append(rallyLine(rally, all.length - stories.length));
  if (stories.length === 0) {
    root.append(el("p", "muted", !all.length ? "No stories have been recorded yet."
      : rally && rally.opened ? "Nothing recorded for this rally's projects since it opened."
        : "No story in the ledger is held in this rally yet."));
    root.append(pastForm(check ? check.head.seq : ledger.as_of));
    return;
  }
  const counts = {};
  for (const [, story] of stories) counts[story.state] = (counts[story.state] || 0) + 1;
  const stack = el("div", "bar tall");
  stack.title = "Stories by state";
  const legend = el("div", "legend");
  for (const [state, name] of STATES) {
    const count = counts[state] || 0;
    if (count) {
      const part = el("div", "s-" + state);
      part.style.flexGrow = String(count);
      part.title = name + ": " + count;
      stack.append(part);
    }
    const key = el("span");
    key.append(el("i", "s-" + state), name + " " + count);
    legend.append(key);
  }
  const heading = el("p", "figure", stories.length
    + (rallyKnown(rally) ? " stories in this rally's projects" : " stories in the ledger"));
  root.append(heading, stack, legend);
  root.append(evidenceGroup(stories, untrusted, now));
  root.append(pastForm(check ? check.head.seq : ledger.as_of));
}

// What's in the current rally and why, or why it couldn't be told (RAL-69), and the past
// rallies, each kept whole (RAL-76).
function rallyLine(rally, otherStories) {
  const box = el("div", "group");
  if (rally && rally.rallies_unavailable) {
    box.append(el("p", "muted", "The service doesn't list rallies yet (" + rally.rallies_unavailable
      + "), so what's in play is read from the roster."));
  } else if (rally && rally.rallies_error) {
    // Not an older service: a rally may be open that this board can't show (RAL-77).
    box.append(el("p", "warning", "⚠ The rallies couldn't be read (" + rally.rallies_error
      + "), so an open rally may be missing; what's in play is read from the roster meanwhile."));
  }
  if (!rallyKnown(rally)) {
    box.append(el("p", "warning", "⚠ Can't tell which projects are in this rally: "
      + ((rally && rally.reason) || "the viewer didn't say.")));
    return box;
  }
  const prefixes = rally.prefixes || [];
  if (rally.opened) {
    const head = el("h3", "", "Rally: " + rally.name);
    head.append(" ", el("span", "label", "declared"));
    box.append(head);
    box.append(el("p", "figure", "Projects: " + prefixes.join(", ")
      + ((rally.teams || []).length ? " · Teams: " + rally.teams.join(", ") : "")));
    // A session restarted under a new nickname, beside the one it succeeds (RAL-108): confirmed
    // by `rally rally-session add --succeeds`, or read from the ledger and roster (RAL-110).
    const successors = rally.successors || [];
    if (successors.length) {
      box.append(el("p", "muted", "Restarted since it opened (declared): "
        + successors.map((s) => s.name + " after " + s.succeeds
          + (s.team ? " (" + s.team + ")" : "")
          + (s.confirmed ? ", confirmed" : ", unconfirmed restart")).join("; ")));
    }
    // Sessions the operator or watchman added or removed since it opened (RAL-110).
    const latest = new Map();
    for (const entry of rally.sessions_changed || []) latest.set(entry.session, entry);
    const added = [...latest.values()].filter((e) => !e.remove).map((e) => e.session);
    const removed = [...latest.values()].filter((e) => e.remove).map((e) => e.session);
    if (added.length || removed.length) {
      box.append(el("p", "muted", "Specified since it opened (declared):"
        + (added.length ? " added " + added.join(", ") : "")
        + (added.length && removed.length ? ";" : "")
        + (removed.length ? " removed " + removed.join(", ") : "")));
    }
    // Projects the operator or watchman added or removed since it opened (RAL-135), each kept.
    const changes = rally.projects_changed || [];
    if (changes.length) {
      box.append(el("p", "muted", "Projects changed since it opened (declared): "
        + changes.map((c) => (c.remove ? "removed " : "added ") + c.project + " at record " + c.seq
          + (c.source && c.source.declared_name ? " by " + c.source.declared_name : "")).join("; ")
        + (rally.opened_with ? " · opened with " + rally.opened_with.join(", ") : "")));
    }
    box.append(el("p", "muted", "Opened " + when(rally.opened_at_ms) + ", at record " + rally.since_seq
      + (rally.opened_by ? ", by " + rally.opened_by : "") + ". " + rally.rule));
  } else {
    const line = el("p", "figure");
    line.append(el("span", "", prefixes.length
      ? "In this rally: " + prefixes.join(", ") : "No project is in this rally yet"));
    line.append(el("span", "label", "observed"));
    box.append(line);
    box.append(el("p", "muted", rally.rule));
  }
  if (otherStories > 0) {
    box.append(el("p", "muted", counted(otherStories, "earlier or other story is", "earlier or other stories are")
      + " kept in the ledger, not shown here."));
  }
  const past = rally.past || [];
  if (past.length) {
    const details = el("details", "sections");
    details.append(el("summary", "", "Past rallies (" + past.length + ")"));
    const list = el("ul", "story-list");
    for (const r of past) {
      const item = el("li");
      const link = el("a", "", r.name);
      link.href = "#/rally/" + encodeURIComponent(r.opened_seq);
      item.append(link, " ", el("span", "muted", (r.projects || []).join(", ") + " · "
        + when(r.opened_at_ms) + " to " + when(r.closed_at_ms)));
      list.append(item);
    }
    details.append(list);
    box.append(details);
  }
  return box;
}

// What Rally's own check of a submission's evidence found (RAL-5), in words.
function checkWords(check) {
  if (!check.checkable || check.exists === undefined || check.exists === null) {
    return check.reason || "not checked";
  }
  if (check.exists === false) return "no such commit";
  const parts = ["commit exists"];
  if (check.names_story === true) parts.push("names the story");
  else if (check.names_story === false) parts.push("doesn't name the story");
  else parts.push("whether it names the story is unknown");
  if (check.ci) parts.push("CI " + check.ci.replace("_", " "));
  if (check.reason) parts.push(check.reason);
  return parts.join(" · ");
}

// Evidence for stories in review or verified (RAL-27): what the team submitted, declared, beside
// Rally's own check of it, observed, each with how old it is. Newest submission first.
function evidenceGroup(stories, untrusted, now) {
  const group = el("div", "group");
  group.append(el("h3", "", "Evidence"));
  const rows = stories
    .filter(([, story]) => story.evidence && (story.state === "in_review" || story.state === "verified"))
    .sort((a, b) => b[1].evidence.at_ms - a[1].evidence.at_ms);
  if (!rows.length) {
    group.append(el("p", "muted", "No story in review or verified has evidence recorded."));
    return group;
  }
  group.append(el("p", "muted", untrusted || UNTRUSTED_FALLBACK));
  // A reader's own correction of its note, or a holder's reply, sits beside the read (RAL-123). The
  // column shows only when some read has one, so the table stays narrow otherwise.
  const notes = rows.some(([, story]) => story.read && (story.read.annotations || []).length);
  const heads = ["Story", "State", "Submitted", "Evidence", "Rally's check"];
  if (notes) heads.push("Notes on the read");
  group.append(table(heads,
    rows.map(([id, story]) => {
      const evidence = story.evidence;
      const check = evidence.check;
      const short = shortCommit(evidence.text);
      const cells = [
        { text: id, className: "nowrap" },
        // A verified state is itself the watchman's record, so it isn't labelled "verified"
        // a second time; a state still in review is only declared (RAL-96).
        { text: statePill(story.state), label: story.state === "verified" ? undefined : "declared",
          className: "nowrap" },
        { text: ago(evidence.at_ms, now), className: "nowrap" },
        short ? { text: short, label: "declared", className: "nowrap" }
          : { text: evidence.text, label: "declared", className: "text" },
        check
          ? { text: checkText(check, now), label: "observed", className: "text" }
          : { text: "not checked yet", className: "muted" },
      ];
      if (notes) cells.push(readNotes(story.read, now));
      return cells;
    })));
  return group;
}

// Rally's check in words with its age, then any correction of it, which is someone's declaration,
// not Rally's observation, and says so (RAL-134).
function checkText(check, now) {
  const parts = [checkWords(check) + " (", ago(check.at_ms, now), ")"];
  for (const fix of check.corrections || []) {
    parts.push(" · corrected by " + (fix.by || "someone") + " (declared): " + fix.note + " (", ago(fix.at_ms, now), ")");
  }
  return parts;
}

// The annotations on a story's latest read, each as "team: note (age)", newest last (RAL-123).
function readNotes(read, now) {
  const annotations = (read && read.annotations) || [];
  if (!annotations.length) return { text: "", className: "muted" };
  const parts = [];
  annotations.forEach((note, i) => {
    if (i) parts.push(" · ");
    parts.push(note.team + ": " + note.note + " (", ago(note.at_ms, now), ")");
  });
  return { text: parts, label: "declared", className: "text" };
}

// Pick a past moment by record number (RAL-28): the page for it is #/past/<record>.
function pastForm(newest) {
  const form = el("form", "past-form");
  const label = el("label", "", "See the ledger as it stood at record ");
  const input = el("input");
  input.type = "number";
  input.min = "1";
  if (newest) input.max = String(newest);
  input.required = true;
  input.name = "as_of";
  label.append(input);
  form.append(label, " ", el("button", "", "Show"));
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const seq = Number(input.value);
    if (Number.isInteger(seq) && seq >= 1) location.hash = "#/past/" + seq;
  });
  return form;
}

let pastCache = null; // { key, value } or { key, error }: a past state doesn't change, so it's read once
let pastShown = null; // the cache entry last drawn, so the 30-second refresh doesn't redraw it


function when(ms) {
  return ms ? new Date(ms).toLocaleString() : "unknown time";
}

// The ledger as it stood at a past record, and what was corrected since, kept apart and labelled;
// or a closed rally's own board, as it stood when it closed (RAL-77).
function renderPast(moment, untrusted) {
  const root = document.getElementById("past-page");
  root.replaceChildren();
  const back = el("a", "back", "← Now");
  back.href = "#/";
  const title = moment.rally ? "A past rally" : "The ledger at record " + moment.seq;
  const waiting = moment.rally ? "Replaying the ledger to the rally's close…"
    : "Replaying the ledger to record " + moment.seq + "…";
  root.append(back, el("h2", "", title));
  if (!pastCache || pastCache.key !== moment.key) {
    root.append(el("p", "muted", waiting));
    const asked = moment.key;
    pastCache = { key: asked, loading: true };
    const query = moment.rally ? "rally=" + encodeURIComponent(moment.seq) : "as_of=" + encodeURIComponent(moment.seq);
    fetch("/api/past?" + query, { cache: "no-store" })
      .then(async (response) => {
        if (!response.ok) throw new Error(await response.text());
        return response.json();
      })
      .then((value) => { if (pastCache.key === asked) pastCache = { key: asked, value }; },
        (error) => { if (pastCache.key === asked) pastCache = { key: asked, error: String(error.message || error) }; })
      .then(() => { const now = currentPast(); if (now && now.key === asked) renderPast(now, untrusted); });
    return;
  }
  if (pastCache.loading) {
    root.append(el("p", "muted", waiting));
    return;
  }
  if (pastCache.error) {
    pastShown = pastCache;
    root.append(el("p", "warning", "⚠ Couldn't show " + (moment.rally ? "that rally" : "record " + moment.seq)
      + ": " + pastCache.error));
    return;
  }
  pastShown = pastCache;
  const past = pastCache.value;
  const rally = past.rally || null;
  if (rally) {
    // Its name, who opened and closed it, and the note were declared by whoever wrote them.
    const head = el("h2", "", "Rally: " + rally.name);
    head.append(" ", el("span", "label", "declared"));
    root.replaceChildren(back, head);
    const line = el("p", "figure", "Projects: " + (rally.prefixes || []).join(", ")
      + ((rally.board.teams || []).length ? " · Teams: " + rally.board.teams.join(", ") : ""));
    line.append(" ", el("span", "label", "declared"));
    root.append(line);
    const opened = el("p", "muted", "Opened " + when(rally.opened_at_ms) + " at record " + rally.since_seq
      + (rally.opened_by ? " by " + rally.opened_by : ""));
    if (rally.opened_by) opened.append(" ", el("span", "label", "declared"));
    root.append(opened);
    const closed = el("p", "muted", "Closed " + when(rally.closed_at_ms) + " at record " + rally.closed_seq
      + (rally.closed_by ? " by " + rally.closed_by : "") + (rally.note ? ": " + rally.note : "."));
    if (rally.closed_by || rally.note) closed.append(" ", el("span", "label", "declared"));
    root.append(closed);
    root.append(el("p", "muted", rally.rule));
    root.append(eventList(rally.events || [], past.untrusted_text || untrusted, Date.now()));
    const whole = el("a", "", "The whole ledger at record " + rally.closed_seq);
    whole.href = "#/past/" + encodeURIComponent(rally.closed_seq);
    const wholeLine = el("p");
    wholeLine.append(whole);
    root.append(wholeLine);
  }
  const onBoard = (id) => !rally || (rally.board.stories || []).includes(id);
  const teamOnBoard = (name) => !rally || (rally.board.teams || []).includes(name);
  root.append(pastForm(past.now));
  root.append(el("p", "muted", "As it stood after record " + past.as_of + ", made " + when(past.as_of_at_ms)
    + ": a replay of the ledger to that record."
    + (past.now ? " The ledger is at record " + past.now + " now. " : " ")
    + (past.untrusted_text || untrusted)));

  const later = el("section", "panel");
  later.append(el("h3", "", "Corrected later"));
  const corrections = past.corrections || [];
  if (past.corrections_unavailable) {
    later.append(el("p", "warning", "⚠ Later corrections couldn't be read, so this may not show everything "
      + "said since about this moment: " + past.corrections_unavailable));
  } else if (corrections.length) {
    later.append(el("p", "muted", "Recorded after record " + past.as_of
      + ", about records at or before it. They weren't known at the time, so they're shown apart."));
    later.append(table(["Record", "Corrects", "About", "Note", "Sets", "By", "When"],
      corrections.map((c) => [
        { text: "#" + c.seq, label: "later" },
        "#" + c.corrects,
        { text: c.story || c.team || "", className: "text" },
        { text: c.note || "", className: "text" },
        c.state || "",
        { text: c.source.credential + " credential, declaring " + (c.source.declared_name || "no name"),
          label: c.kind },
        when(c.at_ms),
      ])));
    if (past.corrections_truncated) {
      later.append(el("p", "warning", "⚠ There are more later corrections than the viewer reads."));
    }
  } else {
    later.append(el("p", "muted", "Nothing recorded since corrects what the ledger held then."));
  }

  const stories = el("section", "panel");
  stories.append(el("h3", "", "Stories then"));
  const rows = Object.entries(past.stories || {}).filter(([id]) => onBoard(id));
  if (rows.length) {
    stories.append(table(["Story", "State", "Team"], rows.map(([id, story]) => [
      id, story.state, { text: story.team || "", className: "text" }])));
  } else {
    stories.append(el("p", "muted", "No stories had been recorded yet."));
  }
  if (past.truncated) {
    stories.append(el("p", "warning", "⚠ More stories or teams than the viewer reads; only the first are shown."));
  }

  const teams = el("section", "panel");
  teams.append(el("h3", "", "Teams then"));
  const teamRows = Object.entries(past.teams || {}).filter(([name]) => teamOnBoard(name));
  if (teamRows.length) {
    teams.append(table(["Team", "State", "Story", "Note", "When"], teamRows.map(([name, team]) => [
      name, { text: team.status, label: "declared" }, team.story || "",
      { text: team.note || "", className: "text" }, when(team.at_ms)])));
  } else {
    teams.append(el("p", "muted", "No team had declared a state yet."));
  }

  const blockers = el("section", "panel");
  blockers.append(el("h3", "", "Open blockers then"));
  const open = (past.blockers || []).filter((b) => !rally || teamOnBoard(b.team) || onBoard(b.story));
  if (open.length) {
    blockers.append(table(["Blocker", "Team", "Story", "Waits on", "Note"], open.map((b) => [
      "#" + b.seq, { text: b.team || "", className: "text" }, b.story || "",
      { text: b.on_story || b.on_team || "", className: "text" }, { text: b.note || "", className: "text" }])));
    if (past.blockers_truncated) blockers.append(el("p", "warning", "⚠ Only the first page of blockers is shown."));
  } else {
    blockers.append(el("p", "muted", "No blockers were open."));
  }
  root.append(later, stories, teams, blockers);
}

// The past moment the address names: #/past/<record>, or #/rally/<record that opened it>.
function currentPast() {
  const match = /^#\/(past|rally)\/([0-9]+)$/.exec(location.hash);
  if (!match) return null;
  return { key: match[1] + ":" + match[2], seq: Number(match[2]), rally: match[1] === "rally" };
}

function renderRoadmaps(projects, now, rally, projectsFile) {
  const root = drawing("roadmaps");
  // RAL-133: the viewer re-reads its projects file when it changes; a version it couldn't use says so.
  if (projectsFile && projectsFile.problem) {
    root.append(el("p", "warning", "⚠ The projects list changed but couldn't be read: " + projectsFile.problem + "."));
  }
  const shown = projects.filter((p) => p.in_rally !== false);
  if (!shown.length) root.append(el("p", "muted", "No listed project is in this rally."));
  const unlisted = (rallyKnown(rally) && rally.prefixes || []).filter((prefix) => !projects.some((p) => p.prefix === prefix));
  if (unlisted.length) {
    root.append(el("p", "warning", "⚠ In this rally without a roadmap in the viewer's project list: "
      + unlisted.join(", ") + "."));
  }
  for (const project of shown) {
    const card = el("article", "roadmap");
    const head = el("div", "roadmap-head");
    const title = el("h3");
    const link = el("a", "", project.name);
    link.href = "#/project/" + encodeURIComponent(project.prefix);
    title.append(link);
    head.append(title, el("span", "muted", project.prefix));
    card.append(head);
    const boxes = project.boxes;
    if (boxes && boxes.total > 0) {
      card.append(figure(
        "Roadmap ticks · a tick here means: " + project.tick_meaning,
        boxes.percent + "% (" + boxes.ticked + " of " + boxes.total + ")",
        boxes.ticked / boxes.total,
        boxes.ticked + " of " + boxes.total + " ticked",
        tickLegend(),
      ));
    } else if (fromLedger(project)) {
      card.append(ledgerFigure(fromLedger(project)));
    } else {
      card.append(figure("Roadmap ticks", "not available", null));
    }
    const verified = project.verified;
    if (verified.available) {
      card.append(figure(
        "Verified · stories the ledger tracks, not the whole roadmap",
        verified.verified + " of " + verified.tracked,
        stateBar(verified, verified.tracked),
        null,
        stateLegend(verified),
      ));
    } else {
      card.append(figure("Verified", "not available yet", null));
    }
    if (project.sections && project.sections.length > 1) {
      const details = el("details", "sections");
      details.append(el("summary", "", "By section"));
      for (const section of project.sections) {
        const row = el("div", "section-row");
        row.append(el("span", "section-name", section.name));
        row.append(bar(section.total ? section.ticked / section.total : 0,
          section.ticked + " of " + section.total));
        row.append(el("span", "", section.ticked + "/" + section.total));
        details.append(row);
      }
      card.append(details);
    }
    const source = el("p", "source muted");
    const stale = project.read_at_ms && now - project.read_at_ms > STALE_MS;
    source.append(project.repo + " " + project.path
      + (project.commit ? " at " + project.commit.slice(0, 7) : "") + ", read ", ago(project.read_at_ms, now));
    card.append(source);
    if (stale) card.append(el("p", "warning", "⚠ This is out of date"));
    if (project.error) card.append(el("p", "warning", "⚠ Last read failed: " + project.error));
    root.append(card);
  }
  const others = projects.filter((p) => p.in_rally === false);
  if (others.length) {
    const details = el("details", "sections");
    details.append(el("summary", "", "Not in this rally (" + others.length + ")"));
    details.append(el("p", "muted", (rally && rally.opened ? "Not named by this rally. "
      : "No team on the roster holds one of their stories. ")
      + "Their records are kept, and each still has its page."));
    const list = el("ul", "story-list");
    for (const project of others) {
      const item = el("li");
      const link = el("a", "", project.name);
      link.href = "#/project/" + encodeURIComponent(project.prefix);
      item.append(link, " ", el("span", "muted", project.prefix));
      list.append(item);
    }
    details.append(list);
    root.append(details);
  }
}

function renderTeams(status, now) {
  const root = drawing("teams");
  const ledger = status.ledger;
  const teams = Object.entries((ledger.available && ledger.teams) || {})
    .filter(([name]) => teamInRally(status.rally, name));
  const group = el("div", "group");
  group.append(el("h3", "", "Declared to the ledger"));
  if (teams.length) {
    group.append(table(["Team", "State", "Story", "Note", "By", "When"], teams.map(([name, team]) => [
      name,
      { text: team.status, label: "declared" },
      team.story || "",
      { text: team.note || "", className: "text" },
      team.source.credential + " credential, declaring " + team.source.declared_name,
      { text: ago(team.at_ms, now), className: "nowrap" },
    ])));
  } else {
    group.append(el("p", "muted", rallyKnown(status.rally)
      ? "No team in this rally has declared a state yet." : "No team has declared a state yet."));
  }
  root.append(group);
  root.append(trackGroup(ledger, now, status.rally));

  const roster = status.roster;
  const live = el("div", "group");
  live.append(el("h3", "", "On the roster"));
  if (roster.available) {
    live.append(rosterSource(roster));
    const rows = roster.sessions.filter((session) => sessionInRally(status.rally, session)).sort((a, b) =>
      String(a.project).localeCompare(String(b.project)));
    // Who each team is, where it was started, what it's on and when it last acted (RAL-99), each
    // fact under the label its source gives it (RAL-116).
    live.append(table(["Team", "Directory", "Project", "Machine", "Status", "Working on", "Last action"],
      rows.map((session) => [
        { text: rosterTeam(session, now), className: "nowrap" },
        { text: rosterDirectory(session.cwd), label: "declared" },
        { text: rosterProject(session.project, session.git_branch), label: "declared" },
        { text: session.machine || "", label: session.machine_label || "declared", className: "nowrap" },
        { text: rosterStatus(session, now), className: "nowrap" },
        { text: rosterWorkingOn(session, now), label: "declared", className: "text" },
        { text: rosterLastAction(session, now), label: "declared", className: "nowrap" },
      ])));
    if (now - roster.read_at_ms > LIVE_STALE_MS) {
      live.append(el("p", "warning figure", "⚠ This roster is out of date"));
    }
    const hidden = roster.sessions.length - rows.length;
    if (hidden > 0) {
      live.append(el("p", "muted figure", counted(hidden, "other session on the roster isn't", "other sessions on the roster aren't")
        + " in this rally."));
    }
    live.append(el("p", "muted figure", ["Read ", ago(roster.read_at_ms, now),
      ". “wake” is a best guess, and none of this proves a session is alive."]));
  } else {
    live.append(el("p", "muted", "Not available yet: " + roster.reason));
  }
  root.append(live);
}

// Each team's track record (RAL-27): counts and dates only, no scores. Verifications and send-backs
// are verified records; problems reported are blockers the team declared.
function trackGroup(ledger, now, rally) {
  const group = el("div", "group");
  group.append(el("h3", "", rally && rally.opened ? "Track records, across all rallies" : "Track records"));
  const tracks = Object.entries((ledger.available && ledger.track_records) || {})
    .filter(([name]) => teamInRally(rally, name));
  if (!tracks.length) {
    group.append(el("p", "muted", !ledger.available ? "Not available yet."
      : rallyKnown(rally) ? "No team in this rally has a track record yet." : "No team has a track record yet."));
    return group;
  }
  const when = (ms) => (ms ? ago(ms, now) : "never");
  group.append(table(["Team", "Verifications", "Send-backs", "Problems reported", "Open"],
    tracks.map(([name, t]) => [
      name,
      { text: [t.verifications + ", last ", when(t.last_verification_at_ms)], label: "verified" },
      { text: [t.send_backs + ", last ", when(t.last_send_back_at_ms)], label: "verified" },
      { text: [t.problems_reported + ", last ", when(t.last_problem_at_ms)], label: "declared" },
      { text: String(t.problems_open), label: "declared" },
    ])));
  if (ledger.track_records_truncated === true) {
    group.append(el("p", "warning", "⚠ More teams than the viewer reads; use rally track-record for the rest."));
  }
  if (ledger.plain_facts) group.append(el("p", "muted figure", ledger.plain_facts));
  if (ledger.problems_reported_are) group.append(el("p", "muted figure", ledger.problems_reported_are));
  group.append(el("p", "muted figure", ["Read ", ago(ledger.read_at_ms, now), ", with the ledger."]));
  return group;
}

// A day summary from docs/muster/, or the watchman's writeup, with where it came from and the
// untrusted notice. Its markdown is drawn as headings, lists, emphasis and code (RAL-94), built
// only with el() and text, so whatever HTML it holds shows as text and nothing in it runs.
function summaryBlock(summary, notice, now) {
  const wrap = el("div");
  if (!summary || summary.reason) {
    wrap.append(el("p", "muted", "Not available yet" + (summary && summary.reason ? ": " + summary.reason : ".")));
    return wrap;
  }
  if (summary.missing) {
    wrap.append(el("p", "muted", "No day summary in " + (summary.local ? summary.folder : "docs/muster/")
      + " yet."));
  } else if (typeof summary.text === "string") {
    wrap.append(el("p", "muted", notice));
    const text = el("div", "summary writeup");
    text.append(...markdownBlocks(summary.text));
    wrap.append(text);
    wrap.append(el("p", "source muted", summary.local
      ? ["Read from the local file " + summary.file + ", ", ago(summary.read_at_ms, now)]
      : [summary.file + " at " + String(summary.commit).slice(0, 7) + ", read ", ago(summary.read_at_ms, now)]));
  }
  if (summary.error) wrap.append(el("p", "warning", "⚠ Last read failed: " + summary.error));
  return wrap;
}

// Markdown, as the writeups use it (RAL-94): headings, paragraphs, lists (nested by indent), quotes,
// rules, fenced code and pipe tables, with inline code, emphasis and links. It makes nodes with
// el() and text only, never by parsing markup, so raw HTML in the text stays text. A link is a link only
// when it's http or https; anything else, javascript: included, is shown as its text.
const MD_FENCE = /^\s*(`{3,}|~{3,})/;
const MD_HEADING = /^(#{1,6})\s+(.*?)\s*#*\s*$/;
const MD_ITEM = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/;
const MD_RULE = /^\s*([-*_])(\s*\1){2,}\s*$/;
const MD_QUOTE = /^\s*>\s?(.*)$/;
const MD_ROW = /^\s*\|(.*)\|\s*$/;
const MD_DIVIDER = /^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?\s*$/;

function markdownBlocks(text) {
  const lines = String(text).replace(/\r\n?/g, "\n").split("\n");
  const blocks = [];
  let i = 0;
  const isBlank = (line) => line.trim() === "";
  const startsBlock = (line) => MD_FENCE.test(line) || MD_HEADING.test(line) || MD_ITEM.test(line)
    || MD_RULE.test(line) || MD_QUOTE.test(line) || MD_ROW.test(line);
  while (i < lines.length) {
    const line = lines[i];
    if (isBlank(line)) { i++; continue; }
    const fence = MD_FENCE.exec(line);
    if (fence) {
      const close = fence[1][0];
      const body = [];
      i++;
      while (i < lines.length && !new RegExp("^\\s*\\" + close + "{" + fence[1].length + ",}\\s*$").test(lines[i])) {
        body.push(lines[i]);
        i++;
      }
      i++;
      const pre = el("pre", "md-code");
      pre.append(el("code", "", body.join("\n")));
      blocks.push(pre);
      continue;
    }
    const heading = MD_HEADING.exec(line);
    if (heading) {
      // The writeup sits under its card's h3, so its own headings start at h4.
      const level = Math.min(6, heading[1].length + 3);
      blocks.push(el("h" + level, "md-heading", markdownInline(heading[2])));
      i++;
      continue;
    }
    if (MD_RULE.test(line)) {
      blocks.push(el("hr"));
      i++;
      continue;
    }
    if (MD_QUOTE.test(line)) {
      const body = [];
      while (i < lines.length && MD_QUOTE.test(lines[i])) {
        body.push(MD_QUOTE.exec(lines[i])[1]);
        i++;
      }
      const quote = el("blockquote");
      quote.append(...markdownBlocks(body.join("\n")));
      blocks.push(quote);
      continue;
    }
    if (MD_ROW.test(line)) {
      const rows = [];
      while (i < lines.length && MD_ROW.test(lines[i])) {
        if (!MD_DIVIDER.test(lines[i])) rows.push(MD_ROW.exec(lines[i])[1].split("|").map((c) => c.trim()));
        i++;
      }
      const wrap = el("div", "table-wrap");
      const table = el("table");
      rows.forEach((cells, n) => {
        const tr = el("tr");
        for (const cell of cells) tr.append(el(n === 0 ? "th" : "td", "", markdownInline(cell)));
        table.append(tr);
      });
      wrap.append(table);
      blocks.push(wrap);
      continue;
    }
    if (MD_ITEM.test(line)) {
      // Items nest by their indent: a deeper item opens a list inside the item before it.
      const stack = [];
      while (i < lines.length && (MD_ITEM.test(lines[i]) || (!isBlank(lines[i]) && stack.length
        && /^\s+\S/.test(lines[i]) && !startsBlock(lines[i])))) {
        const item = MD_ITEM.exec(lines[i]);
        if (!item) {
          // A continuation line belongs to the item above it.
          const last = stack[stack.length - 1].list.lastElementChild;
          last.append(" ", ...markdownInline(lines[i].trim()));
          i++;
          continue;
        }
        const indent = item[1].replace(/\t/g, "    ").length;
        const ordered = /\d/.test(item[2]);
        while (stack.length && indent < stack[stack.length - 1].indent) stack.pop();
        let top = stack[stack.length - 1];
        // A bulleted list after a numbered one at the same depth, or the other way round, is a new list.
        if (top && indent === top.indent && top.ordered !== ordered) {
          stack.pop();
          top = null;
        }
        if (!top || indent > top.indent) {
          const parent = stack[stack.length - 1];
          const list = el(ordered ? "ol" : "ul");
          if (parent && parent.list.lastElementChild) parent.list.lastElementChild.append(list);
          else blocks.push(list);
          top = { indent, ordered, list };
          stack.push(top);
        }
        top.list.append(el("li", "", markdownInline(item[3])));
        i++;
      }
      continue;
    }
    // A paragraph runs to a blank line or the start of another block.
    const body = [];
    while (i < lines.length && !isBlank(lines[i]) && !(body.length && startsBlock(lines[i]))) {
      body.push(lines[i].trim());
      i++;
    }
    blocks.push(el("p", "", markdownInline(body.join(" "))));
  }
  return blocks;
}

// Inline markdown as a list of nodes and strings: `code`, **strong**, *emphasis* or _emphasis_,
// and [text](http or https address). Everything else, any HTML included, is text.
const MD_INLINE = /(`+)([\s\S]*?[^`])\1(?!`)|\*\*(?=\S)([\s\S]*?\S)\*\*|(?<![\w])__(?=\S)([\s\S]*?\S)__(?![\w])|\*(?=\S)([^*]*?\S)\*|(?<![\w])_(?=\S)([^_]*?\S)_(?![\w])|\[([^\]]+)\]\(\s*([^)\s]+)\s*\)/g;

function markdownInline(text, depth) {
  const out = [];
  const nested = (depth || 0) + 1;
  // Deep nesting gives no more meaning, so past a few levels the rest is text.
  if (nested > 4) return [String(text)];
  let last = 0;
  for (const match of String(text).matchAll(MD_INLINE)) {
    if (match.index > last) out.push(text.slice(last, match.index));
    last = match.index + match[0].length;
    if (match[1]) out.push(el("code", "", match[2].trim() || match[2]));
    else if (match[3] || match[4]) out.push(el("strong", "", markdownInline(match[3] || match[4], nested)));
    else if (match[5] || match[6]) out.push(el("em", "", markdownInline(match[5] || match[6], nested)));
    else out.push(markdownLink(match[7], match[8], nested));
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

// A link only to http or https, opened without a referrer or a handle back to this page; any other
// address, javascript: and data: included, shows as the text and the address, inert.
function markdownLink(label, address, depth) {
  let url = null;
  try { url = new URL(address); } catch { /* not an absolute address */ }
  if (!url || (url.protocol !== "http:" && url.protocol !== "https:")) {
    return el("span", "md-link-inert", [...markdownInline(label, depth), " (" + address + ")"]);
  }
  const link = el("a", "", markdownInline(label, depth));
  link.href = url.href;
  link.rel = "noopener noreferrer";
  link.target = "_blank";
  link.referrerPolicy = "no-referrer";
  return link;
}

function storyList(stories) {
  const list = el("ul", "stories");
  for (const story of stories) {
    const item = el("li");
    item.append(el("span", "story-id", story.id || "·"), " ", story.title);
    if (story.last_changed) item.append(" ", el("span", "muted", "last changed " + story.last_changed));
    list.append(item);
  }
  return list;
}

// One roadmap's progress for planning (RAL-23). Shows what couldn't be read rather than zeros, and
// never compares this roadmap with another.
function progressPanel(progress, untrusted, now) {
  const panel = el("section", "panel");
  panel.append(el("h3", "", "Progress"));
  if (!progress) {
    panel.append(el("p", "muted", "Not read yet."));
    return panel;
  }
  if (!progress.roadmap.available) {
    panel.append(el("p", "warning", "⚠ The roadmap couldn't be read: " + progress.roadmap.reason));
  } else if (progress.roadmap.last_read_failed) {
    panel.append(el("p", "warning", ["⚠ The last roadmap read failed (" + progress.roadmap.last_read_failed
      + "), so this uses the one from ", ago(progress.roadmap.read_at_ms, now)]));
  }
  if (!progress.ledger.available) {
    panel.append(el("p", "warning", "⚠ The ledger couldn't be read: " + progress.ledger.reason));
    return panel;
  }
  const states = progress.states;
  panel.append(table(["Ready", "Claimed", "In review", "Verified", "Sent back"],
    [[states.ready, states.claimed, states.in_review, states.verified, states.sent_back]]));
  panel.append(el("p", "muted", progress.tracked + " of this roadmap's stories are in the ledger."));
  panel.append(el("p", "", "Ready with nothing blocking them: "
    + (progress.ready_unblocked.length ? progress.ready_unblocked.join(", ") : "none")));
  if (progress.blockers_truncated) {
    panel.append(el("p", "warning", "⚠ Only the first 200 open blockers were read, so a story above may be "
      + "blocked by one that wasn't."));
  }
  if (progress.top_blockers.length) {
    panel.append(el("p", "muted", "Blockers holding up the most of this roadmap's stories. " + untrusted));
    panel.append(table(["Blocker", "Team", "Holds up", "Note"], progress.top_blockers.map((b) => [
      "#" + b.blocker, { text: b.team || "", className: "text" }, b.holds_up.join(", "),
      { text: b.note || "", className: "text" }])));
  }
  if (progress.blockers_truncated) {
    panel.append(el("p", "warning", "⚠ More open blockers than one page; only the first 200 are counted."));
  }
  const ticks = progress.ticks_without_verified;
  if (ticks) {
    panel.append(el("p", "", ticks.length
      ? "Ticked on the roadmap without a Verified record in the ledger:"
      : "Every ticked story has a Verified record in the ledger."));
    if (ticks.length) {
      panel.append(table(["Story", "Title", "Ledger"], ticks.map((t) => [
        t.id, { text: t.title, className: "text" }, t.ledger_state || "not recorded"])));
    }
  }
  return panel;
}

// One project's page (RAL-43). Built only from /api/status, and only for a project the viewer lists.
function renderProject(status, prefix, now) {
  const root = drawing("project-page");
  const back = el("a", "back", "← All projects");
  back.href = "#/";
  root.append(back);
  const project = status.projects.find((p) => p.prefix === prefix);
  if (!project) {
    root.append(el("h2", "", "Unknown project"), el("p", "muted", "This viewer doesn't list a project with that prefix."));
    return;
  }
  root.append(el("h2", "", project.name + " · " + project.prefix));

  const roadmap = el("section", "panel");
  roadmap.append(el("h3", "", "Roadmap"));
  const stories = project.stories || [];
  if (project.boxes && project.boxes.total > 0) {
    roadmap.append(figure("Ticked · a tick here means: " + project.tick_meaning,
      project.boxes.ticked + " of " + project.boxes.total, project.boxes.ticked / project.boxes.total,
      project.boxes.ticked + " of " + project.boxes.total + " ticked", tickLegend()));
  } else if (fromLedger(project)) {
    roadmap.append(ledgerFigure(fromLedger(project)));
  }
  const open = stories.filter((s) => !s.ticked);
  const done = stories.filter((s) => s.ticked);
  for (const [name, group] of [["Open", open], ["Ticked", done]]) {
    const details = el("details", "sections");
    if (name === "Open") details.open = true;
    details.append(el("summary", "", name + " (" + group.length + ")"));
    details.append(storyList(group));
    roadmap.append(details);
  }
  if (done.some((s) => s.last_changed)) {
    roadmap.append(el("p", "muted", "A ticked story's date is when its line last changed: the tick's date, "
      + "unless the line was edited after it was ticked."));
  }
  if (project.blame && project.blame.error) {
    roadmap.append(el("p", "warning", "⚠ Couldn't read the dates: " + project.blame.error));
  }
  roadmap.append(el("p", "source muted", [project.repo + " " + project.path
    + (project.commit ? " at " + project.commit.slice(0, 7) : "") + ", read ", ago(project.read_at_ms, now)]));
  if (project.error) roadmap.append(el("p", "warning", "⚠ Last read failed: " + project.error));

  root.append(roadmap);
  root.append(progressPanel(project.progress, status.untrusted_text, now));

  const ledger = el("section", "panel");
  ledger.append(el("h3", "", "Ledger records"));
  const records = status.ledger.available && status.ledger.stories
    ? Object.entries(status.ledger.stories).filter(([id]) => id.startsWith(project.prefix + "-")) : [];
  if (records.length) {
    ledger.append(el("p", "muted", status.untrusted_text));
    ledger.append(table(["Story", "State"], records.map(([id, story]) => [id, story.state])));
  } else {
    ledger.append(el("p", "muted", "Not deployed yet: the ledger service comes with RAL-27, and nothing for "
      + project.prefix + " is recorded here."));
  }

  const day = el("section", "panel");
  day.append(el("h3", "", "The team's day"));
  day.append(summaryBlock(project.summary, status.untrusted_text, now));

  const facts = el("section", "panel");
  facts.append(el("h3", "", "From the repository"));
  const merged = project.merged || {};
  if (merged.prs && merged.prs.length) {
    facts.append(el("p", "muted", "Recently merged pull requests. Titles are written by agents: "
      + status.untrusted_text));
    facts.append(table(["PR", "Title", "Merged"], merged.prs.map((pr) => [
      "#" + pr.number, { text: pr.title, className: "text" }, pr.merged_at.slice(0, 10)])));
    facts.append(el("p", "source muted", ["Read ", ago(merged.read_at_ms, now),
      ". Dated ticks aren't shown yet."]));
  } else {
    facts.append(el("p", "muted", "No merged pull requests read yet."));
  }
  if (merged.error) facts.append(el("p", "warning", "⚠ Last read failed: " + merged.error));

  root.append(ledger, day, facts);
}

// A roadmap with no checkboxes, such as a table, takes its progress from the ledger (RAL-81).
function fromLedger(project) {
  return (project.progress && project.progress.from_ledger) || null;
}

function ledgerFigure(part) {
  return figure("Progress from the ledger · this roadmap has no checkboxes, so it's Verified of the stories the ledger tracks",
    part.percent + "% (" + part.verified + " of " + part.tracked + ")",
    stateBar(part, part.tracked), null, stateLegend(part));
}

// The end of the overview: totals across projects, then the watchman's writeup of the whole muster.
function renderMuster(status, now) {
  const root = drawing("muster");
  const current = status.projects.filter((p) => p.in_rally !== false);
  const read = current.filter((p) => p.boxes && p.boxes.total > 0);
  const ticked = read.reduce((sum, p) => sum + p.boxes.ticked, 0);
  const total = read.reduce((sum, p) => sum + p.boxes.total, 0);
  root.append(figure("Roadmap boxes across " + read.length + " projects · ticks mean different things in each",
    ticked + " of " + total, total ? ticked / total : 0, ticked + " of " + total + " ticked", tickLegend()));
  const unboxed = current.filter((p) => fromLedger(p)).map((p) => p.prefix);
  if (unboxed.length) {
    root.append(el("p", "muted", "No checkboxes in " + unboxed.join(", ")
      + ", so not counted here; their progress is the ledger's Verified."));
  }
  const verified = current.map((p) => p.verified).filter((v) => v && v.available);
  if (verified.length) {
    const sum = (state) => verified.reduce((total, v) => total + (v[state] || 0), 0);
    const counts = { verified: sum("verified"), in_review: sum("in_review"), sent_back: sum("sent_back") };
    const tracked = sum("tracked");
    root.append(figure("Verified in the ledger · the sum of each project's Verified", counts.verified + " of " + tracked,
      stateBar(counts, tracked), null, stateLegend(counts)));
  }
  const rally = status.rally;
  if (rally && rally.opened) root.append(eventList(rally.events || [], status.untrusted_text, now));
  const writeup = el("div", "group");
  writeup.append(el("h3", "", "The watchman's writeup"));
  const summary = status.muster_summary || {};
  // Whether it's this rally's is the server's to say, from when the file was written (RAL-104),
  // not from its name's date: a rally opened later the same day doesn't inherit the earlier one's.
  const fit = summary.for_rally;
  if (fit === "before_it_opened") {
    writeup.append(el("p", "muted", "No writeup for this rally yet. The latest, " + summary.file
      + ", was written before it opened."));
  } else if (fit === "time_unknown") {
    writeup.append(el("p", "warning", "⚠ When the latest writeup, " + summary.file
      + ", was written couldn't be read, so it isn't shown as this rally's: "
      + (summary.written_at_error || "no time was given") + "."));
  } else {
    writeup.append(summaryBlock(summary, status.untrusted_text, now));
  }
  // summaryBlock says a failed read itself; the two notes above don't show it.
  if (summary.error && (fit === "before_it_opened" || fit === "time_unknown")) {
    writeup.append(el("p", "warning", "⚠ Last read failed: " + summary.error));
  }
  root.append(writeup);
}

// A rally's major events, newest first (RAL-104): each declared by whoever wrote it, with its
// severity and what backs it.
function eventList(events, untrusted, now) {
  const group = el("div", "group events");
  group.append(el("h3", "", "This rally's events (" + events.length + ")"));
  if (!events.length) {
    group.append(el("p", "muted", "None recorded. Anyone records one with rally event."));
    return group;
  }
  group.append(el("p", "muted", untrusted));
  const list = el("ul", "events");
  for (const event of events) {
    const item = el("li", event.severity === "incident" ? "incident" : "notice");
    item.append(el("span", "label", event.severity === "incident" ? "incident" : "notice"), " ",
      String(event.headline || ""), " ", el("span", "label", event.kind || "declared"));
    const line = el("p", "muted");
    line.append((event.by ? "by " + event.by + ", " : "") + "record " + event.seq + ", ", ago(event.at_ms, now));
    const backing = event.backing || [];
    line.append(backing.length ? " · backed by " + backing.join(", ") : " · nothing named as backing it");
    item.append(line);
    list.append(item);
  }
  group.append(list);
  return group;
}

let lastStatus = null;

// RAL-89: a refresh draws each section into a detached copy, then patches the page's own section to
// match it, so a node that hasn't changed is the same node afterwards. Nothing that didn't change is
// rebuilt: no flash, and focus, selection, scroll and what the reader opened stay where they were.
let drafts = null;

// Where a section's render draws: its detached copy during a patched draw, or the page's own,
// emptied, otherwise.
function drawing(id) {
  if (drafts && drafts[id]) return drafts[id];
  const live = document.getElementById(id);
  live.replaceChildren();
  return live;
}

// A kept node keeps the listeners from its first draw (RAL-113): a handler in a patched section must
// read what it needs when it runs, never close over data or nodes from the render that made it.
function drawPatched(id, render) {
  const live = document.getElementById(id);
  const draft = live.cloneNode(false);
  drafts = { [id]: draft };
  try {
    render();
  } finally {
    drafts = null;
  }
  patchChildren(live, draft);
}

// The same kind of node: text with text, or an element with the same tag.
function sameKind(a, b) {
  return a.nodeType === b.nodeType && (a.nodeType !== Node.ELEMENT_NODE || a.tagName === b.tagName);
}

function patchChildren(live, fresh) {
  const wanted = [...fresh.childNodes];
  wanted.forEach((node, i) => {
    const have = live.childNodes[i];
    if (!have) live.append(node);
    else if (sameKind(have, node)) patchNode(have, node);
    else live.replaceChild(node, have);
  });
  while (live.childNodes.length > wanted.length) live.lastChild.remove();
}

function patchNode(live, fresh) {
  if (live.nodeType !== Node.ELEMENT_NODE) {
    if (live.nodeValue !== fresh.nodeValue) live.nodeValue = fresh.nodeValue;
    return;
  }
  // A section the reader opened or closed stays as they left it (RAL-96).
  const keep = (name) => live.tagName === "DETAILS" && name === "open";
  for (const { name } of [...live.attributes]) {
    if (!fresh.hasAttribute(name) && !keep(name)) live.removeAttribute(name);
  }
  for (const { name, value } of [...fresh.attributes]) {
    if (live.getAttribute(name) !== value && !keep(name)) live.setAttribute(name, value);
  }
  patchChildren(live, fresh);
}

function show(status, now) {
  const match = /^#\/project\/([^/]+)$/.exec(location.hash);
  const pastSeq = currentPast();
  const page = document.getElementById("project-page");
  const overview = document.getElementById("overview");
  const pastPage = document.getElementById("past-page");
  page.hidden = !match;
  pastPage.hidden = pastSeq === null;
  overview.hidden = Boolean(match) || pastSeq !== null;
  if (pastSeq !== null) {
    if (!(pastShown && pastShown === pastCache && pastCache.key === pastSeq.key)) {
      renderPast(pastSeq, status.untrusted_text);
    }
  } else if (match) {
    let prefix = "";
    try { prefix = decodeURIComponent(match[1]); } catch (error) { prefix = ""; }
    drawPatched("project-page", () => renderProject(status, prefix, now));
  } else {
    drawPatched("ledger", () => renderLedger(status.ledger, status.backup, now, status.untrusted_text, status.rally));
    drawPatched("roadmaps", () => renderRoadmaps(status.projects, now, status.rally, status.projects_file));
    drawPatched("teams", () => renderTeams(status, now));
    drawPatched("muster", () => renderMuster(status, now));
  }
}

window.addEventListener("hashchange", () => {
  if (lastStatus) show(lastStatus, Date.now());
  window.scrollTo(0, 0); // a new page starts at its top
});

// When the page last drew what it read, for the live indicator, or why it couldn't (RAL-90).
let drawnAt = null;
let freshnessProblem = null;

// "● Live · updated 3s ago", amber once a poll is late and red once the data is stale; or the
// problem, while there is one.
function showFreshness(now) {
  const freshness = document.getElementById("freshness");
  if (!freshness) return;
  if (freshnessProblem) {
    freshness.className = "muted freshness stale";
    if (freshness.textContent !== freshnessProblem) freshness.textContent = freshnessProblem;
    return;
  }
  if (drawnAt === null) return;
  const late = now - drawnAt;
  const state = late > LIVE_STALE_MS ? "stale" : late > 45000 ? "late" : "live";
  // It's a live region, so only a change of state is announced; the ticking age is hidden from it.
  const words = { live: "Live", late: "Live, a poll is late", stale: "⚠ Not updated for a while" }[state];
  let ticking = freshness.querySelector(".ticking");
  if (freshness.dataset.state !== state || !ticking) {
    freshness.dataset.state = state;
    freshness.className = "muted freshness " + state;
    ticking = el("span", "ticking");
    ticking.setAttribute("aria-hidden", "true");
    freshness.replaceChildren(el("span", "dot", "●"), " " + words, ticking);
  }
  const text = " · updated " + age(drawnAt, now);
  if (ticking.textContent !== text) ticking.textContent = text;
}

// Every second, each age and the live indicator are worked out again, without redrawing anything.
function tick() {
  const now = Date.now();
  for (const node of document.querySelectorAll(".age[data-ts]")) {
    const text = age(Number(node.dataset.ts), now, node.dataset.form);
    if (node.textContent !== text) node.textContent = text;
  }
  showFreshness(now);
}

// A session's start directory, with the home folder as ~ and the whole path in its title (RAL-99).
function rosterDirectory(cwd) {
  if (!cwd) return el("span", "muted", "unknown");
  const short = String(cwd).replace(/^\/(?:home|Users)\/[^/]+(?=\/|$)/, "~");
  const code = el("code", "path", short);
  code.title = String(cwd);
  return code;
}

// The project (its repository's name) and, when known, the branch checked out.
function rosterProject(name, branch) {
  const span = el("span");
  span.append(name || "none");
  if (branch) span.append(" ", el("span", "muted", "(" + branch + ")"));
  return span;
}

// Busy or idle, as the session declares it, and whether a message would wake it,
// observed only when the daemon holds its wait.
// Which commands the roster was read with, and why any weren't (RAL-116, RAL-126): the panel shows
// every source's sessions together.
function rosterSource(roster) {
  const sources = roster.sources || (roster.source ? [roster.source] : []);
  const line = el("p", "muted figure", "Read with " + (sources.length
    ? sources.map((s) => s + " roster").join(" and ") : "an unnamed command") + ".");
  for (const passed of roster.passed_over || []) {
    line.append(" ", el("span", "roster-fallback", passed.command + "'s roster " + passed.reason
      + ", so its sessions aren't shown."));
  }
  return line;
}

// The roster a row came from, shortened to the command's name, and any other it's also on.
function sourceName(command) {
  return String(command || "").split("/").pop();
}

// A fact with its label beside it, for a cell holding facts under more than one label.
function labelled(label, ...content) {
  const span = el("span");
  span.append(el("span", "label", label), " ", ...content);
  return span;
}

// The team's nickname, and when its session connected, as the session declared it.
function rosterTeam(session, now) {
  const span = el("span");
  span.append(session.nickname || "");
  if (session.source) {
    const also = (session.also_on || []).map(sourceName);
    span.append(el("br"), el("span", "muted roster-source", "via " + sourceName(session.source)
      + (also.length ? ", also on " + also.join(", ") : "")));
  }
  if (session.connected_at) {
    span.append(el("br"), el("span", "muted", [labelled("declared", "connected ", ago(session.connected_at, now))]));
  }
  return span;
}

// What the session says it's doing and for how long, its wake, and what the daemon holds about its
// connection and wait, each under its source's label: Muster's observed, Marshal's recorded (RAL-116).
function rosterStatus(session, now) {
  const span = el("span", "status");
  // How long it's been busy or idle comes only from when the shim declared that it changed
  // (`activity_at`, RAL-124). The last tool or prompt isn't that, so without it Rally says so.
  const activity = [session.activity || "unknown"];
  if (session.activity) {
    if (session.activity_at) activity.push(" for ", ago(session.activity_at, now, "for"));
    else activity.push(el("span", "muted", ", since unknown"));
  }
  span.append(labelled("declared", ...activity));
  if (session.wake) {
    span.append(el("br"), el("span", "muted", [labelled(session.wake_label || "declared", "wake: " + session.wake)]));
  }
  const connection = session.connection;
  if (connection) {
    const facts = [connection.bound_to_connection ? "bound to a connection" : "not bound to a connection"];
    if (connection.waiting && connection.waiting_until) {
      facts.push(", waiting until ", el("span", "", new Date(connection.waiting_until).toLocaleTimeString()));
    } else if (connection.waiting) {
      facts.push(", waiting");
    }
    span.append(el("br"), el("span", "muted", [labelled(connection.label, ...facts)]));
  }
  return span;
}

// The declared task, and how long ago it was set when the shim says.
function rosterWorkingOn(session, now) {
  const span = el("span");
  span.append(session.current_task || "");
  if (session.current_task && session.current_task_at) {
    span.append(" ", el("span", "muted", ["· set ", ago(session.current_task_at, now)]));
  }
  return span;
}

// When the session last did anything, and the last tool it called.
function rosterLastAction(session, now) {
  const span = el("span");
  span.append(ago(session.last_activity_at, now));
  if (session.last_tool) {
    const tool = el("span", "muted", session.last_tool);
    if (session.last_tool_at) tool.append(" · ", ago(session.last_tool_at, now));
    span.append(el("br"), tool);
  }
  return span;
}

// A story's state as a dot in the bar's colour and its name (RAL-96).
function statePill(state) {
  const pill = el("span", "state-pill");
  pill.append(el("span", "dot s-" + state), state.replace("_", " "));
  return pill;
}

// Which collapsible sections are open, by their summary text, so the 30-second
// redraw doesn't snap them shut (RAL-96).
function openSections() {
  return new Set([...document.querySelectorAll("details[open] > summary")].map((s) => s.textContent));
}

function reopen(open) {
  for (const summary of document.querySelectorAll("details > summary")) {
    if (open.has(summary.textContent)) summary.parentElement.open = true;
  }
}

// A redraw would lose what someone is typing or has selected; the next refresh
// draws what this one read (RAL-96).
function busyReading() {
  const active = document.activeElement;
  if (active && active.matches("input, textarea, select")) return true;
  const selection = window.getSelection();
  return Boolean(selection && !selection.isCollapsed);
}

// The page code this tab is running, stamped into the page it was loaded from (RAL-112).
const PAGE_CODE = (document.querySelector('meta[name="rally-page-code"]') || {}).content || null;

// When the viewer now serves other page code than this tab runs, as after a restart onto a new
// build, reload once to run it: a tab left open across a deploy otherwise keeps drawing new data
// with old code. Not while the operator is reading or typing, and at most once for each code, so
// a page that keeps getting a mismatch can't reload forever (RAL-112). `reload` is for the browser
// test, which checks the decision without navigating away.
function reloadForNewCode(status, reload = () => location.reload()) {
  const serving = status && status.page_code;
  if (!PAGE_CODE || !serving || serving === PAGE_CODE || busyReading()) return false;
  try {
    if (sessionStorage.getItem("rally-reloaded-for") === serving) return false;
    sessionStorage.setItem("rally-reloaded-for", serving);
  } catch (error) {
    return false;
  }
  reload();
  return true;
}

// RAL-138: reactive cues. Each cue marks one real change between two polls (a new record, a story
// whose state or check changed, a count that moved, a session going busy or idle) and then goes:
// nothing animates while nothing changes. With prefers-reduced-motion the cue is a still outline for
// the same few seconds instead of a movement (app.css).
const CUE_MS = 2400;
let cueBefore = null;

function cueSnapshot(status) {
  const stories = {};
  for (const [id, story] of Object.entries((status.ledger && status.ledger.stories) || {})) {
    const check = story.evidence && story.evidence.check;
    stories[id] = { state: story.state, check: JSON.stringify(check === undefined ? null : check) };
  }
  const sessions = {};
  for (const session of (status.roster && status.roster.sessions) || []) {
    if (session.nickname) sessions[session.nickname] = session.activity || "";
  }
  return { seq: status.ledger ? status.ledger.as_of : null, stories, sessions };
}

// What the board's legend says now, by state, to tell which counts a redraw moved.
function legendCounts() {
  const counts = {};
  for (const key of document.querySelectorAll("#ledger .legend span")) {
    const swatch = key.querySelector("i");
    if (swatch) counts[swatch.className] = key.textContent;
  }
  return counts;
}

// Mark `node` for one cue. The colour is set with setProperty, which the page's CSP allows.
function cue(node, kind, colour) {
  if (!node) return;
  if (colour) node.style.setProperty("--cue", colour);
  node.classList.remove("cue-" + kind);
  void node.offsetWidth; // so the same cue can play again on the next change
  node.classList.add("cue-" + kind);
  clearTimeout(node.cueTimer);
  node.cueTimer = setTimeout(() => node.classList.remove("cue-" + kind), CUE_MS);
}

// A story's row in the board's Evidence table, or its line in a project page's story list.
function storyRow(id) {
  return [...document.querySelectorAll("#ledger tbody tr")]
    .find((row) => row.cells[0] && row.cells[0].textContent.trim() === id)
    || [...document.querySelectorAll(".stories li")]
      .find((line) => { const key = line.querySelector(".story-id"); return key && key.textContent.trim() === id; });
}

function sessionRow(nickname) {
  return [...document.querySelectorAll("tbody tr")].find((row) => {
    const first = row.cells[0] && row.cells[0].querySelector("span");
    return first && first.firstChild && first.firstChild.nodeValue === nickname;
  });
}

function cueChanges(status, legendBefore) {
  const now = cueSnapshot(status);
  const before = cueBefore;
  cueBefore = now;
  if (!before) return; // the first draw: nothing has changed yet
  if (now.seq !== before.seq) {
    cue(document.getElementById("freshness"), "arrive");
    cue(document.querySelector(".sigil"), "arrive");
  }
  for (const [id, story] of Object.entries(now.stories)) {
    const was = before.stories[id];
    if (!was || was.state !== story.state || was.check !== story.check) {
      cue(storyRow(id), "changed", "var(--s-" + story.state + ")");
    }
  }
  const legendNow = legendCounts();
  for (const key of document.querySelectorAll("#ledger .legend span")) {
    const swatch = key.querySelector("i");
    if (swatch && legendBefore[swatch.className] !== undefined
      && legendBefore[swatch.className] !== legendNow[swatch.className]) cue(key, "count");
  }
  for (const [nickname, activity] of Object.entries(now.sessions)) {
    if (before.sessions[nickname] !== undefined && before.sessions[nickname] !== activity) {
      cue(sessionRow(nickname), "activity", activity === "busy" ? "var(--arc-2)" : "var(--text-2)");
    }
  }
}

async function refresh() {
  let status;
  try {
    const response = await fetch("/api/status", { cache: "no-store" });
    if (!response.ok) throw new Error("status " + response.status);
    status = await response.json();
  } catch (error) {
    freshnessProblem = "⚠ Can't reach the viewer, so this may be out of date";
    showFreshness(Date.now());
    // Don't leave an earlier green check standing when nothing can back it up.
    const ledger = document.getElementById("ledger");
    ledger.replaceChildren(el("p", "warning", "⚠ The viewer isn't answering, so the ledger's state is unknown"));
    return;
  }
  if (reloadForNewCode(status)) return;
  try {
    const now = Date.now();
    lastStatus = status;
    // "Updated" says when the page was last drawn, so it moves only when it is.
    if (!busyReading()) {
      const open = openSections();
      const legendBefore = legendCounts();
      show(status, now);
      cueChanges(status, legendBefore);
      reopen(open);
      drawnAt = now;
    }
    freshnessProblem = null;
  } catch (error) {
    freshnessProblem = "⚠ This page couldn't display what it read, so it may be out of date";
  }
  showFreshness(Date.now());
}

refresh();
setInterval(refresh, 30000);
setInterval(tick, 1000);
