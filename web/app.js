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
  return [session.nickname, session.project].some((n) => n && teams.includes(String(n).trim().toLowerCase()));
}

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function age(ms, now) {
  if (ms === undefined || ms === null) return "unknown";
  const seconds = Math.max(0, Math.round((now - ms) / 1000));
  if (seconds < 60) return seconds + "s ago";
  if (seconds < 3600) return Math.round(seconds / 60) + "m ago";
  if (seconds < 172800) return Math.round(seconds / 3600) + "h ago";
  return Math.round(seconds / 86400) + "d ago";
}

function bar(fraction, title, tall) {
  const track = el("div", tall ? "bar tall" : "bar");
  track.title = title;
  if (fraction > 0) {
    const fill = el("div");
    fill.style.width = Math.min(100, fraction * 100) + "%";
    track.append(fill);
  }
  return track;
}

function figure(label, value, fraction, title) {
  const wrap = el("div", "figure");
  const line = el("div", "figure-label");
  line.append(el("span", "", label), el("strong", "", value));
  wrap.append(line);
  if (fraction !== null) wrap.append(bar(fraction, title));
  return wrap;
}

function table(headers, rows) {
  const wrap = el("div", "table-wrap");
  const tbl = el("table");
  const head = el("tr");
  for (const header of headers) head.append(el("th", "", header));
  const thead = el("thead");
  thead.append(head);
  const tbody = el("tbody");
  for (const row of rows) {
    const tr = el("tr");
    for (const cell of row) {
      const td = el("td", cell.className || "");
      if (cell.label) td.append(el("span", "label", cell.label), " ");
      td.append(cell.text === undefined ? String(cell) : cell.text);
      tr.append(td);
    }
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
  const where = "record " + backup.seq + ", saved " + age(backup.saved_at_ms, now);
  if (backup.overdue) {
    root.append(el("p", "warning", "⚠ Backup overdue: the last reached " + where
      + ", and more than " + backup.max_age_hours + " hours is overdue"));
  } else {
    root.append(el("p", "muted", "Backed up on this machine to " + where));
  }
}

function renderLedger(ledger, backup, now, untrusted, rally) {
  const root = document.getElementById("ledger");
  root.replaceChildren();
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
    root.append(el("p", "warning", "⚠ Not read since " + age(ledger.read_at_ms, now)
      + ", so everything here may be out of date"));
  }
  if (check) {
    // A green pass is only shown while the reading behind it is fresh.
    root.append(el("p", check.ok ? (stale ? "muted" : "good") : "warning",
      check.ok ? (stale ? "Ledger check last passed" : "Ledger check passed")
        : "✕ The ledger failed its check"));
    for (const problem of check.problems || []) root.append(el("p", "warning", problem));
    for (const note of check.notes || []) root.append(el("p", "muted", note));
    root.append(el("p", "muted", "Record " + check.head.seq + ", read " + age(ledger.read_at_ms, now)
      + ". This check can't show a rewrite of the whole host without a saved head."));
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
      + "), so what's in play is read from Muster's roster."));
  } else if (rally && rally.rallies_error) {
    // Not an older service: a rally may be open that this board can't show (RAL-77).
    box.append(el("p", "warning", "⚠ The rallies couldn't be read (" + rally.rallies_error
      + "), so an open rally may be missing; what's in play is read from Muster's roster meanwhile."));
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
    box.append(el("p", "muted", otherStories + " earlier or other stories are kept in the ledger, not shown here."));
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
  group.append(table(["Story", "State", "Submitted", "Evidence", "Rally's check"],
    rows.map(([id, story]) => {
      const evidence = story.evidence;
      const check = evidence.check;
      return [
        id,
        { text: story.state.replace("_", " "), label: story.state === "verified" ? "verified" : "declared" },
        age(evidence.at_ms, now),
        { text: evidence.text, label: "declared", className: "text" },
        check
          ? { text: checkWords(check) + " (" + age(check.at_ms, now) + ")", label: "observed", className: "text" }
          : { text: "not checked yet", className: "muted" },
      ];
    })));
  return group;
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

// The local calendar day of a time, as YYYY-MM-DD. Day files are named by the day where they
// were written, so they're compared with a local date, not a UTC one (RAL-77): an evening rally
// west of Greenwich is already tomorrow in UTC.
function localDay(ms) {
  const d = new Date(ms);
  const two = (n) => String(n).padStart(2, "0");
  return d.getFullYear() + "-" + two(d.getMonth() + 1) + "-" + two(d.getDate());
}

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

function renderRoadmaps(projects, now, rally) {
  const root = document.getElementById("roadmaps");
  root.replaceChildren();
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
        verified.tracked ? verified.verified / verified.tracked : 0,
        verified.verified + " of " + verified.tracked + " verified",
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
    source.textContent = project.repo + " " + project.path
      + (project.commit ? " at " + project.commit.slice(0, 7) : "")
      + ", read " + age(project.read_at_ms, now);
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
      : "No team on Muster's roster holds one of their stories. ")
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
  const root = document.getElementById("teams");
  root.replaceChildren();
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
      age(team.at_ms, now),
    ])));
  } else {
    group.append(el("p", "muted", rallyKnown(status.rally)
      ? "No team in this rally has declared a state yet." : "No team has declared a state yet."));
  }
  root.append(group);
  root.append(trackGroup(ledger, now, status.rally));

  const roster = status.roster;
  const live = el("div", "group");
  live.append(el("h3", "", "On Muster's roster"));
  if (roster.available) {
    const rows = roster.sessions.filter((session) => sessionInRally(status.rally, session)).sort((a, b) =>
      String(a.project).localeCompare(String(b.project)));
    live.append(table(["Team", "Session", "Machine", "Can be woken", "Working on", "Activity", "Last active"],
      rows.map((session) => [
        session.project || "",
        session.nickname || "",
        { text: session.machine || "", label: "attested" },
        { text: session.wake || "", label: session.wake_observed ? "observed" : "declared" },
        { text: session.current_task || "", label: "declared", className: "text" },
        { text: session.activity || "", label: "declared" },
        { text: age(session.last_activity_at, now), label: "declared" },
      ])));
    if (now - roster.read_at_ms > LIVE_STALE_MS) {
      live.append(el("p", "warning figure", "⚠ This roster is out of date"));
    }
    const hidden = roster.sessions.length - rows.length;
    if (hidden > 0) live.append(el("p", "muted figure", hidden + " other sessions on the roster aren't in this rally."));
    live.append(el("p", "muted figure", "Read " + age(roster.read_at_ms, now)
      + ". “Can be woken” is a best guess, and none of this proves a session is alive."));
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
  const when = (ms) => (ms ? age(ms, now) : "never");
  group.append(table(["Team", "Verifications", "Send-backs", "Problems reported", "Open"],
    tracks.map(([name, t]) => [
      name,
      { text: t.verifications + ", last " + when(t.last_verification_at_ms), label: "verified" },
      { text: t.send_backs + ", last " + when(t.last_send_back_at_ms), label: "verified" },
      { text: t.problems_reported + ", last " + when(t.last_problem_at_ms), label: "declared" },
      { text: String(t.problems_open), label: "declared" },
    ])));
  if (ledger.track_records_truncated === true) {
    group.append(el("p", "warning", "⚠ More teams than the viewer reads; use rally track-record for the rest."));
  }
  if (ledger.plain_facts) group.append(el("p", "muted figure", ledger.plain_facts));
  if (ledger.problems_reported_are) group.append(el("p", "muted figure", ledger.problems_reported_are));
  group.append(el("p", "muted figure", "Read " + age(ledger.read_at_ms, now) + ", with the ledger."));
  return group;
}

// A day summary from docs/muster/, as text only, with where it came from and the untrusted notice.
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
    wrap.append(el("pre", "summary", summary.text));
    wrap.append(el("p", "source muted", summary.local
      ? "Read from the local file " + summary.file + ", " + age(summary.read_at_ms, now)
      : summary.file + " at " + String(summary.commit).slice(0, 7) + ", read " + age(summary.read_at_ms, now)));
  }
  if (summary.error) wrap.append(el("p", "warning", "⚠ Last read failed: " + summary.error));
  return wrap;
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
    panel.append(el("p", "warning", "⚠ The last roadmap read failed (" + progress.roadmap.last_read_failed
      + "), so this uses the one from " + age(progress.roadmap.read_at_ms, now)));
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
  const root = document.getElementById("project-page");
  root.replaceChildren();
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
      project.boxes.ticked + " of " + project.boxes.total + " ticked"));
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
  roadmap.append(el("p", "source muted", project.repo + " " + project.path
    + (project.commit ? " at " + project.commit.slice(0, 7) : "") + ", read " + age(project.read_at_ms, now)));
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
    facts.append(el("p", "source muted", "Read " + age(merged.read_at_ms, now)
      + ". Dated ticks aren't shown yet."));
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
    part.tracked ? part.verified / part.tracked : 0,
    part.verified + " of " + part.tracked + " verified");
}

// The end of the overview: totals across projects, then the watchman's writeup of the whole muster.
function renderMuster(status, now) {
  const root = document.getElementById("muster");
  root.replaceChildren();
  const current = status.projects.filter((p) => p.in_rally !== false);
  const read = current.filter((p) => p.boxes && p.boxes.total > 0);
  const ticked = read.reduce((sum, p) => sum + p.boxes.ticked, 0);
  const total = read.reduce((sum, p) => sum + p.boxes.total, 0);
  root.append(figure("Roadmap boxes across " + read.length + " projects · ticks mean different things in each",
    ticked + " of " + total, total ? ticked / total : 0, ticked + " of " + total + " ticked"));
  const unboxed = current.filter((p) => fromLedger(p)).map((p) => p.prefix);
  if (unboxed.length) {
    root.append(el("p", "muted", "No checkboxes in " + unboxed.join(", ")
      + ", so not counted here; their progress is the ledger's Verified."));
  }
  const verified = current.map((p) => p.verified).filter((v) => v && v.available);
  if (verified.length) {
    const done = verified.reduce((sum, v) => sum + v.verified, 0);
    const tracked = verified.reduce((sum, v) => sum + v.tracked, 0);
    root.append(figure("Verified in the ledger · the sum of each project's Verified", done + " of " + tracked, tracked ? done / tracked : 0,
      done + " of " + tracked + " verified"));
  }
  const writeup = el("div", "group");
  writeup.append(el("h3", "", "The watchman's writeup"));
  const summary = status.muster_summary || {};
  const day = /([0-9]{4}-[0-9]{2}-[0-9]{2})\.md$/.exec(summary.file || "");
  const rally = status.rally;
  if (rally && rally.opened && day && day[1] < localDay(rally.opened_at_ms)) {
    writeup.append(el("p", "muted", "No writeup for this rally yet. The latest, " + summary.file
      + ", is from before it opened."));
  } else {
    writeup.append(summaryBlock(status.muster_summary, status.untrusted_text, now));
  }
  root.append(writeup);
}

let lastStatus = null;

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
    renderProject(status, prefix, now);
  } else {
    renderLedger(status.ledger, status.backup, now, status.untrusted_text, status.rally);
    renderRoadmaps(status.projects, now, status.rally);
    renderTeams(status, now);
    renderMuster(status, now);
  }
}

window.addEventListener("hashchange", () => {
  if (lastStatus) show(lastStatus, Date.now());
  window.scrollTo(0, 0); // a new page starts at its top
});

async function refresh() {
  const freshness = document.getElementById("freshness");
  let status;
  try {
    const response = await fetch("/api/status", { cache: "no-store" });
    if (!response.ok) throw new Error("status " + response.status);
    status = await response.json();
  } catch (error) {
    freshness.textContent = "⚠ Can't reach the viewer, so this may be out of date";
    // Don't leave an earlier green check standing when nothing can back it up.
    const ledger = document.getElementById("ledger");
    ledger.replaceChildren(el("p", "warning", "⚠ The viewer isn't answering, so the ledger's state is unknown"));
    return;
  }
  try {
    const now = Date.now();
    lastStatus = status;
    show(status, now);
    freshness.textContent = "Updated " + new Date(now).toLocaleTimeString();
  } catch (error) {
    freshness.textContent = "⚠ This page couldn't display what it read, so it may be out of date";
  }
}

refresh();
setInterval(refresh, 30000);
