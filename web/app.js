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
const LIVE_STALE_MS = 3 * 60 * 1000;

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

function renderLedger(ledger, backup, now) {
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
  const stories = Object.entries(ledger.stories || {});
  if (stories.length === 0) {
    root.append(el("p", "muted", "No stories have been recorded yet."));
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
  const heading = el("p", "figure", stories.length + " stories in the ledger");
  root.append(heading, stack, legend);
}

function renderRoadmaps(projects, now) {
  const root = document.getElementById("roadmaps");
  root.replaceChildren();
  for (const project of projects) {
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
}

function renderTeams(status, now) {
  const root = document.getElementById("teams");
  root.replaceChildren();
  const ledger = status.ledger;
  const teams = Object.entries((ledger.available && ledger.teams) || {});
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
    group.append(el("p", "muted", "No team has declared a state yet."));
  }
  root.append(group);

  const roster = status.roster;
  const live = el("div", "group");
  live.append(el("h3", "", "On Muster's roster"));
  if (roster.available) {
    const rows = [...roster.sessions].sort((a, b) =>
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
    live.append(el("p", "muted figure", "Read " + age(roster.read_at_ms, now)
      + ". “Can be woken” is a best guess, and none of this proves a session is alive."));
  } else {
    live.append(el("p", "muted", "Not available yet: " + roster.reason));
  }
  root.append(live);
}

// A day summary from docs/muster/, as text only, with where it came from and the untrusted notice.
function summaryBlock(summary, notice, now) {
  const wrap = el("div");
  if (!summary || summary.reason) {
    wrap.append(el("p", "muted", "Not available yet" + (summary && summary.reason ? ": " + summary.reason : ".")));
    return wrap;
  }
  if (summary.missing) {
    wrap.append(el("p", "muted", "No day summary in docs/muster/ yet."));
  } else if (typeof summary.text === "string") {
    wrap.append(el("p", "muted", notice));
    wrap.append(el("pre", "summary", summary.text));
    wrap.append(el("p", "source muted", summary.file + " at " + String(summary.commit).slice(0, 7)
      + ", read " + age(summary.read_at_ms, now)));
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

// The end of the overview: totals across projects, then the watchman's writeup of the whole muster.
function renderMuster(status, now) {
  const root = document.getElementById("muster");
  root.replaceChildren();
  const read = status.projects.filter((p) => p.boxes && p.boxes.total > 0);
  const ticked = read.reduce((sum, p) => sum + p.boxes.ticked, 0);
  const total = read.reduce((sum, p) => sum + p.boxes.total, 0);
  root.append(figure("Roadmap boxes across " + read.length + " projects · ticks mean different things in each",
    ticked + " of " + total, total ? ticked / total : 0, ticked + " of " + total + " ticked"));
  const verified = status.projects.map((p) => p.verified).filter((v) => v && v.available);
  if (verified.length) {
    const done = verified.reduce((sum, v) => sum + v.verified, 0);
    const tracked = verified.reduce((sum, v) => sum + v.tracked, 0);
    root.append(figure("Verified in the ledger · the sum of each project's Verified", done + " of " + tracked, tracked ? done / tracked : 0,
      done + " of " + tracked + " verified"));
  }
  const writeup = el("div", "group");
  writeup.append(el("h3", "", "The watchman's writeup"));
  writeup.append(summaryBlock(status.muster_summary, status.untrusted_text, now));
  root.append(writeup);
}

let lastStatus = null;

function show(status, now) {
  const match = /^#\/project\/([^/]+)$/.exec(location.hash);
  const page = document.getElementById("project-page");
  const overview = document.getElementById("overview");
  page.hidden = !match;
  overview.hidden = Boolean(match);
  if (match) {
    let prefix = "";
    try { prefix = decodeURIComponent(match[1]); } catch (error) { prefix = ""; }
    renderProject(status, prefix, now);
  } else {
    renderLedger(status.ledger, status.backup, now);
    renderRoadmaps(status.projects, now);
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
