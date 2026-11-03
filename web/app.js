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

function renderLedger(ledger, now) {
  const root = document.getElementById("ledger");
  root.replaceChildren();
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
    head.append(el("h3", "", project.name), el("span", "muted", project.prefix));
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

async function refresh() {
  const freshness = document.getElementById("freshness");
  let status;
  try {
    const response = await fetch("/api/status", { cache: "no-store" });
    if (!response.ok) throw new Error("status " + response.status);
    status = await response.json();
  } catch (error) {
    freshness.textContent = "⚠ Can't reach the viewer, so this may be out of date";
    return;
  }
  try {
    const now = Date.now();
    renderLedger(status.ledger, now);
    renderRoadmaps(status.projects, now);
    renderTeams(status, now);
    freshness.textContent = "Updated " + new Date(now).toLocaleTimeString();
  } catch (error) {
    freshness.textContent = "⚠ This page couldn't display what it read, so it may be out of date";
  }
}

refresh();
setInterval(refresh, 30000);
