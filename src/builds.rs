//! What's live, what's staged and what's merged but not live, for `rally brief --watchman`
//! (RAL-167).
//!
//! Every part is observed, from its own source: the service's commit is the service's own word
//! (`/v1/whoami`, RAL-156), this CLI's is its own build, and main's tip and the commits between
//! are GitHub's. The commits merged but not live are mapped to stories by the story ID in each
//! commit's message, as a merge commit names its branch (`ral-148-digest-scope`) or a commit its
//! story (`RAL-148: ...`). A story waits on the service only when what it brought to main changes a file
//! the service runs (RAL-195): the CLI, the viewer and the service are one binary, so being merged
//! after the service's build doesn't by itself mean waiting on a restart. "Staged" is a hint, not a fact: the CLI on this machine being ahead of
//! the service usually means a build is installed and waiting on a restart. Nothing reads the
//! lamplighter's rollout files.

use std::collections::HashMap;

use serde_json::{Value, json};

/// Rally's own repository on GitHub, from `Cargo.toml`.
pub fn own_repo() -> Option<&'static str> {
    env!("CARGO_PKG_REPOSITORY").strip_prefix("https://github.com/")
}

/// Every Rally story ID a commit message names, in order, once each: `RAL-148`, `ral-148` or
/// `ral148` (as a branch such as `ral147/checks-403` has it).
pub fn stories_in(message: &str) -> Vec<String> {
    let lower = message.to_lowercase();
    let bytes = lower.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while let Some(at) = lower[i..].find("ral") {
        let start = i + at;
        // Not inside a word, such as "general" or "rally".
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let mut j = start + 3;
        if bytes.get(j) == Some(&b'-') {
            j += 1;
        }
        let digits_start = j;
        while bytes.get(j).is_some_and(u8::is_ascii_digit) {
            j += 1;
        }
        if before_ok && j > digits_start {
            let id = format!("RAL-{}", &lower[digits_start..j]);
            if !out.contains(&id) {
                out.push(id);
            }
        }
        i = start + 3;
    }
    out
}

/// GitHub's commit date, `2026-11-13T20:10:00Z`, as milliseconds since 1970.
fn iso_ms(text: &str) -> Option<i64> {
    let text = text.strip_suffix('Z')?;
    crate::inbox::expiry_ms(&format!("{} UTC", text.replacen('T', " ", 1)))
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

/// Which running part of Rally a changed file belongs to (RAL-195). One binary holds the service
/// (`rally serve`), the viewer (`rally web`) and the CLI, so a commit merged after the service's
/// build isn't thereby waiting on the service: only one that changes code the service runs is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Code the service runs: a merged change here isn't live until the service restarts on it.
    Service,
    /// A file the service shares with the CLI or the viewer, where the path alone can't say
    /// which of them a change is for.
    Shared,
    /// Code only the CLI or the viewer runs: live wherever they're on a build that has it.
    Client,
    /// Docs, tests and the rest: nothing running changes.
    Neither,
}

/// The modules the service runs: `server` and what its routes reach. `tests::every_module_is_placed`
/// keeps this, [`SHARED_MODULES`] and [`CLIENT_MODULES`] covering every file in `src/`.
pub const SERVICE_MODULES: &[&str] = &[
    "access",
    "brief",
    "designation",
    "entity",
    "evidence",
    "gate_log",
    "inbox",
    "ledger",
    "next",
    "record",
    "secrets",
    "serve",
    "server",
    "store",
    "views",
    "watchman_brief",
];

/// The files every part shares: `lib` names each module; `web` is the viewer and also the roadmap
/// parser the service's evidence checks use.
pub const SHARED_MODULES: &[&str] = &["lib", "web"];

/// The modules only the CLI or the viewer runs. `main` is among them since RAL-196: it holds
/// every command but the service's, which it passes on whole to `serve`
/// (`tests::main_only_passes_serve_on` keeps it so).
pub const CLIENT_MODULES: &[&str] = &[
    "backup",
    "builds",
    "client",
    "digest",
    "digest_loop",
    "gate",
    "gate_check",
    "in_flight",
    "init",
    "levi",
    "lock_holder",
    "main",
    "merge_notes",
    "rebased",
    "resume",
    "retry",
    "roadmaps",
    "scratch_pg",
    "script",
    "session",
    "shims",
    "slot_queue",
    "stall",
    "view",
    "wait_green",
    "wake",
];

/// The part a changed path belongs to. A source file this build doesn't know (one added since)
/// is [`Part::Shared`]: it can't be said not to be the service's.
pub fn part_of(path: &str) -> Part {
    // The build itself: every part is built from these.
    if matches!(path, "Cargo.toml" | "Cargo.lock" | "build.rs") {
        return Part::Service;
    }
    // The viewer's pages, built into the binary.
    if path.starts_with("web/") {
        return Part::Client;
    }
    let Some(inner) = path.strip_prefix("src/") else {
        return Part::Neither;
    };
    let module = inner
        .split('/')
        .next()
        .unwrap_or_default()
        .trim_end_matches(".rs");
    if SERVICE_MODULES.contains(&module) {
        Part::Service
    } else if CLIENT_MODULES.contains(&module) {
        Part::Client
    } else {
        Part::Shared
    }
}

/// Whether a patch to `src/lib.rs` only adds or removes the names of modules the service doesn't
/// run, as most stories' patches to it do: then it changes nothing the service runs.
fn lib_patch_is_client_only(patch: &str) -> bool {
    let mut changed = 0;
    for line in patch.lines() {
        let Some(text) = line.strip_prefix('+').or_else(|| line.strip_prefix('-')) else {
            continue;
        };
        let module = text
            .trim()
            .strip_prefix("pub mod ")
            .and_then(|rest| rest.strip_suffix(';'));
        match module {
            Some(module) if CLIENT_MODULES.contains(&module) => changed += 1,
            _ => return false,
        }
    }
    changed > 0
}

/// The part one changed file, `{filename, patch}` as GitHub lists it, belongs to.
fn part_of_file(file: &Value) -> Part {
    let path = file["filename"].as_str().unwrap_or_default();
    match (path, file["patch"].as_str()) {
        ("src/lib.rs", Some(patch)) if lib_patch_is_client_only(patch) => Part::Client,
        _ => part_of(path),
    }
}

/// The files each commit changes, by sha, or why that commit's couldn't be read.
pub type CommitFiles = HashMap<String, Result<Value, String>>;

/// What's read for the `builds` section, each from its own source.
#[derive(Default)]
pub struct Read<'a> {
    /// The service's commit, if it says.
    pub live: Option<&'a str>,
    /// This build's commit.
    pub cli: Option<&'a str>,
    /// Main's tip.
    pub main: Option<Result<String, String>>,
    /// GitHub's compare of live…main: `{status, ahead_by, commits: [{sha, message, date}]}`.
    pub behind: Option<Result<Value, String>>,
    /// GitHub's compare of live…cli.
    pub staged: Option<Result<Value, String>>,
    /// GitHub's compare of cli…main, when they differ: what this CLI doesn't have yet.
    pub cli_behind: Option<Result<Value, String>>,
    /// The files each commit of `behind` changes, by sha, each `[{filename, patch}]` (RAL-195), or
    /// why none were read.
    pub files: Option<Result<CommitFiles, String>>,
    /// The viewer `rally view` started on this machine: its build, or why there's none to name.
    pub viewer: Option<Result<String, String>>,
    pub now_ms: i64,
}

/// The first lines of a compare's commits name these stories, each once.
fn stories_named(compare: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for commit in compare["commits"].as_array().into_iter().flatten() {
        let first = commit["message"]
            .as_str()
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or_default();
        for id in stories_in(first) {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// One commit on main's own line (its first-parent chain) between the service's build and main's
/// tip: a pull request's merge, or a commit made straight on main. GitHub lists a merge's files
/// against its first parent, so these commits' files together are exactly what changed on main,
/// each change once: a pull request's commits are covered by its merge whatever their messages
/// say, and a merge of main into a branch, which repeats what main already had, isn't read at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Landing {
    pub sha: String,
    /// The first line of its message.
    pub title: String,
    /// The stories it names, and those named by the commits it brought to main. Empty when
    /// none of them names one: it's then reported by its commit.
    pub stories: Vec<String>,
}

fn first_line(commit: &Value) -> &str {
    commit["message"]
        .as_str()
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or_default()
}

/// What landed on main, oldest first, from a compare whose commits each say their `parents`.
/// Where they don't, or a commit is reached from no commit on main's line, each such commit
/// stands for itself, so none is left unread.
pub fn landings(compare: &Value) -> Vec<Landing> {
    let commits: Vec<&Value> = compare["commits"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let sha = |commit: &Value| commit["sha"].as_str().unwrap_or_default().to_owned();
    let alone = |commit: &Value| Landing {
        sha: sha(commit),
        title: first_line(commit).to_owned(),
        stories: stories_in(first_line(commit)),
    };
    let parents = |commit: &Value| -> Option<Vec<String>> {
        Some(
            commit["parents"]
                .as_array()?
                .iter()
                .filter_map(|p| p.as_str().map(str::to_owned))
                .collect(),
        )
    };
    if commits.iter().any(|commit| parents(commit).is_none()) {
        return commits.iter().map(|commit| alone(commit)).collect();
    }
    let by_sha: HashMap<String, &Value> = commits.iter().map(|c| (sha(c), *c)).collect();
    // Main's line: from the newest commit back along first parents, while still in the range.
    let mut line: Vec<&Value> = Vec::new();
    let mut at = commits.last().copied();
    while let Some(commit) = at {
        line.push(commit);
        at = parents(commit)
            .and_then(|p| p.first().cloned())
            .and_then(|first| by_sha.get(&first).copied());
    }
    line.reverse();
    let on_line: Vec<String> = line.iter().map(|c| sha(c)).collect();
    let mut taken: Vec<String> = Vec::new();
    let mut out: Vec<Landing> = Vec::new();
    for commit in &line {
        let mut landing = alone(commit);
        // The commits it brought: reached through its other parents, short of main's line.
        let mut todo: Vec<String> = parents(commit)
            .unwrap_or_default()
            .into_iter()
            .skip(1)
            .collect();
        while let Some(next) = todo.pop() {
            let Some(brought) = by_sha.get(&next) else {
                continue;
            };
            if on_line.contains(&next) || taken.contains(&next) {
                continue;
            }
            for id in stories_in(first_line(brought)) {
                if !landing.stories.contains(&id) {
                    landing.stories.push(id);
                }
            }
            todo.extend(parents(brought).unwrap_or_default());
            taken.push(next);
        }
        out.push(landing);
    }
    for commit in &commits {
        let sha = sha(commit);
        if !on_line.contains(&sha) && !taken.contains(&sha) {
            out.push(alone(commit));
        }
    }
    out
}

/// The commits whose files say what changed on main: every [`Landing`]'s. An error, with no
/// commit named, when GitHub lists fewer commits than the service is behind, or more landed than
/// `most`: the files then can't say everything that changed, and the section counts every story
/// as it did before RAL-195.
pub fn commits_to_read(compare: &Value, most: usize) -> Result<Vec<String>, String> {
    let listed = compare["commits"].as_array().map_or(0, Vec::len);
    let ahead = compare["ahead_by"].as_u64().unwrap_or(listed as u64);
    if ahead > listed as u64 {
        return Err(format!(
            "GitHub lists only {listed} of the {ahead} commits, so their files can't say \
             everything that changed"
        ));
    }
    let shas: Vec<String> = landings(compare).into_iter().map(|l| l.sha).collect();
    if shas.len() > most {
        return Err(format!(
            "{} commits landed on main, more than the {most} whose files are read",
            shas.len()
        ));
    }
    Ok(shas)
}

/// One story merged since the service's build.
struct Merged {
    story: String,
    at: i64,
    sha: String,
    service: Vec<String>,
    shared: Vec<String>,
    client: bool,
    unread: Option<String>,
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// The `builds` section, from what was [`Read`].
pub fn section(read: Read) -> Value {
    let Read {
        live,
        cli,
        main,
        behind,
        staged,
        cli_behind,
        files,
        viewer,
        now_ms,
    } = read;
    let mut out = json!({
        "kind": "observed",
        "live": match live {
            Some(sha) => json!({"commit": sha, "from": "the service's /v1/whoami"}),
            None => json!({"commit": null,
                "note": "older than RAL-156: the service doesn't say its commit"}),
        },
        "cli": cli,
        "main": match &main {
            Some(Ok(sha)) => json!(sha),
            Some(Err(why)) => json!({"unread": why}),
            None => Value::Null,
        },
    });
    // Where the CLI and the viewer stand (RAL-195): a story that changes only them is live
    // wherever they're on a build that has it, whatever the service runs.
    let main_tip = main.as_ref().and_then(|m| m.as_ref().ok());
    out["cli_build"] = match (cli, main_tip, &cli_behind) {
        (None, _, _) => json!({"commit": null,
            "note": "this CLI was built from a checkout with uncommitted changes: no commit describes it"}),
        (Some(cli), Some(tip), _) if cli == tip => json!({"commit": cli, "behind_main": 0,
            "summary": format!("this CLI ({}) is main's tip", short(cli))}),
        (Some(cli), _, Some(Ok(compare))) => {
            let lacks = stories_named(compare);
            let behind_by = compare["ahead_by"].as_u64().unwrap_or(0);
            json!({"commit": cli, "behind_main": behind_by, "lacks": lacks,
            "summary": if behind_by == 0 {
                format!("this CLI ({}) has everything on main", short(cli))
            } else if lacks.is_empty() {
                format!("this CLI ({}) is {behind_by} commits behind main; they name no story",
                    short(cli))
            } else {
                format!("this CLI ({}) is {behind_by} commits behind main and lacks {}",
                    short(cli), lacks.join(", "))
            }})
        }
        (Some(cli), _, Some(Err(why))) => json!({"commit": cli, "unread": why}),
        (Some(cli), _, None) => json!({"commit": cli}),
    };
    out["viewer_build"] = match &viewer {
        None => Value::Null,
        Some(Err(why)) => json!({"build": null, "note": why}),
        Some(Ok(build)) => {
            let commit = build.strip_prefix("commit:");
            json!({"build": build, "from": "view-run.json on this machine",
            "summary": match (commit, cli) {
                (Some(viewer), Some(cli)) if viewer == cli =>
                    format!("the viewer here runs this CLI's build ({})", short(cli)),
                (Some(viewer), Some(_)) => format!(
                    "the viewer here runs {}, not this CLI's build: `rally view` restarts it",
                    short(viewer)),
                (Some(viewer), None) => format!("the viewer here runs {}", short(viewer)),
                (None, _) => "the viewer here runs a build no commit describes".to_owned(),
            }})
        }
    };
    // Whether anything merged may wait on the service: unless it's known that nothing does, a
    // CLI ahead of the service hints at a staged build.
    let mut may_wait = true;
    match behind {
        None => {}
        Some(Err(why)) => out["merged_not_live"] = json!({"unread": why}),
        Some(Ok(compare)) => {
            let commits = compare["commits"].as_array().cloned().unwrap_or_default();
            let (files, files_unread) = match files {
                Some(Ok(files)) => (Some(files), None),
                Some(Err(why)) => (None, Some(why)),
                None => (None, Some("the commits' files weren't read".to_owned())),
            };
            // Each story once, at its oldest commit between live and main.
            let mut stories: Vec<Merged> = Vec::new();
            let blank = |story: &str, at: i64, sha: &str| Merged {
                story: story.to_owned(),
                at,
                sha: sha.to_owned(),
                service: Vec::new(),
                shared: Vec::new(),
                client: false,
                unread: None,
            };
            let when = |commit: &Value| commit["date"].as_str().and_then(iso_ms).unwrap_or(now_ms);
            for commit in &commits {
                let at = when(commit);
                let sha = commit["sha"].as_str().unwrap_or_default();
                for id in stories_in(first_line(commit)) {
                    match stories.iter_mut().find(|row| row.story == id) {
                        Some(row) if at < row.at => {
                            row.at = at;
                            row.sha = sha.to_owned();
                        }
                        Some(_) => {}
                        None => stories.push(blank(&id, at, sha)),
                    }
                }
            }
            // What each landing on main changed goes to the stories it names or brought; one
            // that names none is a row of its own, so no change to main goes unread (RAL-195).
            let mut unnamed: Vec<Merged> = Vec::new();
            if let Some(files) = &files {
                for landing in landings(&compare) {
                    let mut change = blank("", 0, "");
                    match files.get(&landing.sha) {
                        Some(Ok(list)) => {
                            for file in list.as_array().into_iter().flatten() {
                                let path = file["filename"].as_str().unwrap_or_default().to_owned();
                                match part_of_file(file) {
                                    Part::Service => change.service.push(path),
                                    Part::Shared => change.shared.push(path),
                                    Part::Client => change.client = true,
                                    Part::Neither => {}
                                }
                            }
                        }
                        Some(Err(why)) => change.unread = Some(why.clone()),
                        None => {
                            change.unread =
                                Some(format!("{}'s files weren't read", short(&landing.sha)));
                        }
                    }
                    let add = |row: &mut Merged| {
                        for path in &change.service {
                            if !row.service.contains(path) {
                                row.service.push(path.clone());
                            }
                        }
                        for path in &change.shared {
                            if !row.shared.contains(path) {
                                row.shared.push(path.clone());
                            }
                        }
                        row.client |= change.client;
                        if change.unread.is_some() {
                            row.unread.clone_from(&change.unread);
                        }
                    };
                    if landing.stories.is_empty() {
                        let at = commits
                            .iter()
                            .find(|c| c["sha"] == landing.sha.as_str())
                            .map_or(now_ms, when);
                        let mut row = blank(&landing.title, at, &landing.sha);
                        add(&mut row);
                        unnamed.push(row);
                    } else {
                        for id in &landing.stories {
                            if let Some(row) = stories.iter_mut().find(|row| row.story == *id) {
                                add(row);
                            }
                        }
                    }
                }
            }
            unnamed.sort_by_key(|row| row.at);
            stories.sort_by_key(|row| row.at);
            // GitHub's compare lists at most 250 commits: further behind, this reads short.
            let ahead = compare["ahead_by"].as_u64().unwrap_or(commits.len() as u64);
            let short_read = (ahead > commits.len() as u64).then(|| {
                format!(
                    "the service is {ahead} commits behind main, but GitHub's compare lists only \
                     {}: the stories and the oldest age here read short",
                    commits.len()
                )
            });
            // Which part each story waits on. With no files read, every story counts as waiting
            // on the service, as before RAL-195, and `files_unread` says why.
            let on = |row: &Merged| -> &'static str {
                if files.is_none() || !row.service.is_empty() {
                    "service"
                } else if row.unread.is_some() || !row.shared.is_empty() {
                    "can't tell"
                } else if row.client {
                    "cli or viewer"
                } else {
                    "nothing running"
                }
            };
            let waiting: Vec<&Merged> = stories.iter().filter(|r| on(r) == "service").collect();
            let unsure: Vec<&Merged> = stories.iter().filter(|r| on(r) == "can't tell").collect();
            let elsewhere = stories.len() - waiting.len() - unsure.len();
            // Commits naming no story: only those that may wait on the service are listed.
            let unnamed_waiting: Vec<&Merged> =
                unnamed.iter().filter(|r| on(r) == "service").collect();
            let unnamed_unsure: Vec<&Merged> =
                unnamed.iter().filter(|r| on(r) == "can't tell").collect();
            let oldest_ms = waiting
                .iter()
                .chain(&unnamed_waiting)
                .map(|row| now_ms.saturating_sub(row.at))
                .max();
            let count = waiting.len() + unnamed_waiting.len();
            may_wait = count > 0 || !unsure.is_empty() || !unnamed_unsure.is_empty();
            // "2 stories and 1 commit naming no story".
            let both = |stories: usize, commits: usize| -> String {
                let mut parts = Vec::new();
                if stories > 0 || commits == 0 {
                    parts.push(plural(stories, "story", "stories"));
                }
                if commits > 0 {
                    parts.push(plural(
                        commits,
                        "commit naming no story",
                        "commits naming no story",
                    ));
                }
                parts.join(" and ")
            };
            let summary = if stories.is_empty() {
                "live is main's tip: nothing merged waits to go live".to_owned()
            } else if files.is_none() {
                format!(
                    "{} merged since the service's build, the oldest {} h ago; which of them \
                     change what the service runs wasn't read",
                    plural(count, "story", "stories"),
                    oldest_ms.unwrap_or(0) / 3_600_000
                )
            } else {
                let mut parts = vec![if count == 0 {
                    "nothing merged since the service's build is known to change what the \
                     service runs"
                        .to_owned()
                } else {
                    format!(
                        "{} merged, not live on the service; the oldest for {} h",
                        both(waiting.len(), unnamed_waiting.len()),
                        oldest_ms.unwrap_or(0) / 3_600_000
                    )
                }];
                if !unsure.is_empty() || !unnamed_unsure.is_empty() {
                    let mut shared: Vec<&str> = Vec::new();
                    for path in unsure
                        .iter()
                        .chain(&unnamed_unsure)
                        .flat_map(|row| &row.shared)
                    {
                        if !shared.contains(&path.as_str()) {
                            shared.push(path);
                        }
                    }
                    parts.push(format!(
                        "{} can't be told from the paths{}: read the diff",
                        both(unsure.len(), unnamed_unsure.len()),
                        if shared.is_empty() {
                            String::new()
                        } else {
                            format!(" (shared with the service: {})", shared.join(", "))
                        }
                    ));
                }
                if elsewhere > 0 {
                    parts.push(format!(
                        "{} only the CLI, the viewer, docs or tests: live wherever those builds \
                         have {} (see cli_build and viewer_build)",
                        if elsewhere == 1 {
                            "1 story changes".to_owned()
                        } else {
                            format!("{elsewhere} stories change")
                        },
                        if elsewhere == 1 { "it" } else { "them" },
                    ));
                }
                parts.join("; ")
            };
            out["merged_not_live"] = json!({
                "commits": ahead,
                "listed": commits.len(),
                "reads_short": short_read,
                "files_unread": files_unread,
                "stories": stories.iter().map(|row| json!({
                    "story": row.story, "commit": short(&row.sha),
                    "age_ms": now_ms.saturating_sub(row.at),
                    "waits_on": on(row),
                    "service_files": row.service,
                    "shared_files": row.shared,
                    "unread": row.unread,
                })).collect::<Vec<_>>(),
                "unnamed": unnamed.iter().filter(|row| matches!(on(row), "service" | "can't tell"))
                    .map(|row| json!({
                        "commit": short(&row.sha), "message": row.story,
                        "age_ms": now_ms.saturating_sub(row.at),
                        "waits_on": on(row),
                        "service_files": row.service,
                        "shared_files": row.shared,
                        "unread": row.unread,
                    })).collect::<Vec<_>>(),
                "merged": stories.len(),
                "count": count,
                "cant_tell": unsure.len() + unnamed_unsure.len(),
                "not_the_service": elsewhere,
                "oldest_age_ms": oldest_ms,
                "summary": summary,
                "how": "the files of each commit on main's own line since the service's build \
                        (a pull request's merge lists all it changed) go to the stories it names \
                        or brought, or to `unnamed` when it names none; a file the service runs \
                        (builds::SERVICE_MODULES, Cargo.toml, Cargo.lock, build.rs) waits on the \
                        service; src/web.rs and most of src/lib.rs are shared, so \
                        the path can't tell",
            });
        }
    }
    if may_wait
        && let Some(Ok(compare)) = staged
        && compare["ahead_by"].as_u64().is_some_and(|n| n > 0)
    {
        out["hint"] = json!(format!(
            "this CLI ({}) is ahead of the service ({}): if the commits between change what the \
             service runs, a build is staged, waiting on a restart [a hint, not a fact]",
            short(cli.unwrap_or_default()),
            short(live.unwrap_or_default())
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commit_message_names_its_stories() {
        assert_eq!(
            stories_in("Merge pull request #232 from Castor-o7/ral-148-digest-scope"),
            ["RAL-148"]
        );
        assert_eq!(
            stories_in("Merge pull request #231 from Castor-o7/ral147/checks-403"),
            ["RAL-147"]
        );
        assert_eq!(
            stories_in("RAL-157 part 1: `rally brief --watchman`"),
            ["RAL-157"]
        );
        assert_eq!(
            stories_in("general rally fixes for RAL-9 and ral-9"),
            ["RAL-9"]
        );
        assert!(stories_in("Merge remote-tracking branch 'origin/main'").is_empty());
    }

    fn files(list: &[(&str, &[&str])]) -> CommitFiles {
        list.iter()
            .map(|(sha, paths)| {
                let files: Vec<Value> = paths.iter().map(|p| json!({"filename": p})).collect();
                (sha.repeat(40), Ok(json!(files)))
            })
            .collect()
    }

    /// RAL-167: the section says what's live, counts the stories merged and not live with the
    /// oldest's age, and hints at a staged build when this CLI is ahead of the service.
    #[test]
    fn the_section_counts_whats_merged_and_not_live() {
        let hour = 3_600_000;
        let now = iso_ms("2026-11-14T04:00:00Z").unwrap();
        let behind = json!({"status": "ahead", "ahead_by": 4, "commits": [
            {"sha": "a".repeat(40), "message": "RAL-148: digest\n\nbody", "date": "2026-11-13T21:00:00Z"},
            {"sha": "b".repeat(40), "message": "Merge pull request #232 from o/ral-148-x", "date": "2026-11-13T22:00:00Z"},
            {"sha": "c".repeat(40), "message": "RAL-149: late checks", "date": "2026-11-14T01:00:00Z"},
            {"sha": "d".repeat(40), "message": "Merge remote-tracking branch 'origin/main'", "date": "2026-11-14T02:00:00Z"},
        ]});
        let out = section(Read {
            live: Some(&"0".repeat(40)),
            cli: Some(&"9".repeat(40)),
            main: Some(Ok("d".repeat(40))),
            behind: Some(Ok(behind)),
            staged: Some(Ok(json!({"ahead_by": 2}))),
            files: Some(Ok(files(&[
                ("a", &["src/store.rs", "tests/service.rs"]),
                ("b", &["src/store.rs", "src/server.rs"]),
                ("c", &["src/evidence.rs"]),
                ("d", &[]),
            ]))),
            now_ms: now,
            ..Read::default()
        });
        let merged = &out["merged_not_live"];
        assert_eq!(merged["count"], 2);
        assert_eq!(merged["commits"], 4);
        assert!(merged["reads_short"].is_null(), "all four were listed");
        assert_eq!(merged["stories"][0]["story"], "RAL-148");
        assert_eq!(merged["stories"][0]["age_ms"], 7 * hour);
        assert_eq!(merged["stories"][0]["waits_on"], "service");
        assert_eq!(
            merged["stories"][0]["service_files"],
            json!(["src/store.rs", "src/server.rs"])
        );
        assert_eq!(merged["oldest_age_ms"], 7 * hour);
        assert_eq!(
            merged["summary"],
            "2 stories merged, not live on the service; the oldest for 7 h"
        );
        assert!(
            out["hint"]
                .as_str()
                .unwrap()
                .starts_with("this CLI (9999999) is ahead")
        );

        // Further behind than GitHub lists: said, not hidden.
        let long = json!({"status": "ahead", "ahead_by": 300, "commits": [
            {"sha": "a".repeat(40), "message": "RAL-1: x", "date": "2026-11-13T21:00:00Z"},
        ]});
        let far = section(Read {
            live: Some("0"),
            main: Some(Ok("d".into())),
            behind: Some(Ok(long)),
            now_ms: now,
            ..Read::default()
        });
        assert_eq!(
            far["merged_not_live"]["reads_short"],
            "the service is 300 commits behind main, but GitHub's compare lists only 1: the \
             stories and the oldest age here read short"
        );

        // A service older than RAL-156 says nothing, and that's said.
        let old = section(Read {
            cli: Some("9"),
            main: Some(Ok("d".into())),
            now_ms: now,
            ..Read::default()
        });
        assert!(
            old["live"]["note"]
                .as_str()
                .unwrap()
                .contains("doesn't say")
        );
        assert!(old.get("merged_not_live").is_none() && old.get("hint").is_none());
        // Live at main's tip.
        let even = json!({"status": "identical", "ahead_by": 0, "commits": []});
        let level = section(Read {
            live: Some("d"),
            cli: Some("d"),
            main: Some(Ok("d".into())),
            behind: Some(Ok(even)),
            staged: Some(Ok(json!({"ahead_by": 0}))),
            files: Some(Ok(HashMap::new())),
            now_ms: now,
            ..Read::default()
        });
        assert_eq!(
            level["merged_not_live"]["summary"],
            "live is main's tip: nothing merged waits to go live"
        );
        assert!(level.get("hint").is_none());
        assert_eq!(level["cli_build"]["summary"], "this CLI (d) is main's tip");
    }

    /// RAL-195: a story merged since the service's build waits on the service only when its
    /// commits change what the service runs. The 2026-11-15 reading, "8 stories merged, not live",
    /// was of commits that changed the CLI, the viewer, docs and tests.
    #[test]
    fn only_what_the_service_runs_waits_on_the_service() {
        let now = iso_ms("2026-11-15T12:00:00Z").unwrap();
        let commit = |sha: &str, message: &str, hour: u32| {
            json!({"sha": sha.repeat(40), "message": message,
                   "date": format!("2026-11-15T{hour:02}:00:00Z")})
        };
        let behind = json!({"status": "ahead", "ahead_by": 6, "commits": [
            commit("a", "RAL-186: wait-green docs", 1),
            commit("b", "RAL-187: the wait-green line", 2),
            commit("c", "RAL-189: test scripts", 3),
            commit("d", "RAL-190: a browser test", 4),
            commit("e", "RAL-193: the inbox folds away", 5),
            commit("f", "RAL-200: a new route", 6),
        ]});
        let mut read = files(&[
            ("a", &["docs/watchman.md", "src/session.rs"]),
            ("b", &["src/web.rs", "src/session.rs"]),
            (
                "d",
                &[
                    "tests/browser/sign-in.mjs",
                    "tests/web.rs",
                    "docs/roadmap.md",
                ],
            ),
            ("e", &["web/app.js", "web/app.css"]),
            ("f", &["src/server.rs", "src/web.rs"]),
        ]);
        // A patch to lib.rs that only names a module the service doesn't run changes nothing
        // the service runs.
        read.insert(
            "c".repeat(40),
            Ok(json!([
                {"filename": "src/script.rs"},
                {"filename": "src/lib.rs", "patch": "@@ -52,6 +52,7 @@\n pub mod scratch_pg;\n+pub mod script;\n pub mod secrets;"},
            ])),
        );
        let input = |files, staged| Read {
            live: Some("0000000000"),
            cli: Some("9999999999"),
            main: Some(Ok("f".repeat(40))),
            behind: Some(Ok(behind.clone())),
            staged,
            cli_behind: Some(Ok(json!({"ahead_by": 2, "commits": [
                commit("e", "RAL-193: the inbox folds away", 5),
                commit("f", "RAL-200: a new route", 6),
            ]}))),
            files,
            viewer: Some(Ok("commit:9999999999".into())),
            now_ms: now,
        };
        let out = section(input(
            Some(Ok(read.clone())),
            Some(Ok(json!({"ahead_by": 4}))),
        ));
        let merged = &out["merged_not_live"];
        let waits: Vec<(&str, &str)> = merged["stories"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["story"].as_str().unwrap(),
                    s["waits_on"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            waits,
            [
                ("RAL-186", "cli or viewer"),
                ("RAL-187", "can't tell"),
                ("RAL-189", "cli or viewer"),
                ("RAL-190", "nothing running"),
                ("RAL-193", "cli or viewer"),
                ("RAL-200", "service"),
            ]
        );
        assert_eq!(
            (
                &merged["merged"],
                &merged["count"],
                &merged["cant_tell"],
                &merged["not_the_service"]
            ),
            (&json!(6), &json!(1), &json!(1), &json!(4))
        );
        // The age is the oldest that waits on the service, not the oldest merged.
        assert_eq!(merged["oldest_age_ms"], 6 * 3_600_000);
        assert_eq!(
            merged["summary"],
            "1 story merged, not live on the service; the oldest for 6 h; 1 story can't be told \
             from the paths (shared with the service: src/web.rs): read the diff; 4 stories \
             change only the CLI, the viewer, docs or tests: live wherever those builds have \
             them (see cli_build and viewer_build)"
        );
        assert_eq!(
            out["cli_build"]["summary"],
            "this CLI (9999999) is 2 commits behind main and lacks RAL-193, RAL-200"
        );
        assert_eq!(
            out["viewer_build"]["summary"],
            "the viewer here runs this CLI's build (9999999)"
        );
        assert!(out["hint"].is_string(), "a story waits on the service");

        // Nothing of the service's changed: nothing waits on it, and a CLI ahead of the service
        // isn't a build staged for it.
        read.remove(&"f".repeat(40));
        read.insert("f".repeat(40), Ok(json!([{"filename": "docs/brief.md"}])));
        read.insert("b".repeat(40), Ok(json!([{"filename": "src/session.rs"}])));
        let calm = section(input(
            Some(Ok(read.clone())),
            Some(Ok(json!({"ahead_by": 4}))),
        ));
        assert_eq!(calm["merged_not_live"]["count"], 0);
        assert!(calm["merged_not_live"]["oldest_age_ms"].is_null());
        assert_eq!(
            calm["merged_not_live"]["summary"],
            "nothing merged since the service's build is known to change what the service runs; \
             6 stories change only the CLI, the viewer, docs or tests: live wherever those \
             builds have them (see cli_build and viewer_build)"
        );
        assert!(calm.get("hint").is_none());

        // A commit whose files couldn't be read isn't called live: it can't be told.
        read.insert("a".repeat(40), Err("gh: timed out".into()));
        let partial = section(input(Some(Ok(read)), None));
        assert_eq!(
            partial["merged_not_live"]["stories"][0]["waits_on"],
            "can't tell"
        );
        assert_eq!(
            partial["merged_not_live"]["stories"][0]["unread"],
            "gh: timed out"
        );

        // No files read at all: every story counts, as before, and it says the files weren't read.
        let blind = section(input(
            Some(Err("more than 60 commits name a story".into())),
            None,
        ));
        assert_eq!(blind["merged_not_live"]["count"], 6);
        assert_eq!(
            blind["merged_not_live"]["files_unread"],
            "more than 60 commits name a story"
        );
        assert_eq!(
            blind["merged_not_live"]["summary"],
            "6 stories merged since the service's build, the oldest 11 h ago; which of them \
             change what the service runs wasn't read"
        );
    }

    /// RAL-195, sent back at 4cbfbc7: a commit that names no story is read too. One that
    /// changes what the service runs counts as waiting on the service, by its commit, and keeps
    /// the staged hint; one that changes only shared files can't be told; one that changes only
    /// docs is nothing to report.
    #[test]
    fn a_commit_naming_no_story_is_read_too() {
        let now = iso_ms("2026-11-15T12:00:00Z").unwrap();
        let commit = |sha: &str, message: &str, hour: u32| {
            json!({"sha": sha.repeat(40), "message": message,
                   "date": format!("2026-11-15T{hour:02}:00:00Z")})
        };
        let behind = json!({"status": "ahead", "ahead_by": 4, "commits": [
            commit("a", "RAL-186: wait-green docs", 1),
            commit("b", "address review", 2),
            commit("c", "tidy the wording", 3),
            commit("d", "a flag for every command", 4),
        ]});
        let input = |read| Read {
            live: Some("0000000000"),
            cli: Some("9999999999"),
            main: Some(Ok("d".repeat(40))),
            behind: Some(Ok(behind.clone())),
            staged: Some(Ok(json!({"ahead_by": 4}))),
            files: Some(Ok(read)),
            now_ms: now,
            ..Read::default()
        };
        let out = section(input(files(&[
            ("a", &["docs/watchman.md"]),
            ("b", &["src/server.rs", "tests/service.rs"]),
            ("c", &["docs/brief.md"]),
            ("d", &["src/web.rs"]),
        ])));
        let merged = &out["merged_not_live"];
        assert_eq!(merged["stories"][0]["waits_on"], "nothing running");
        assert_eq!(
            merged["unnamed"],
            json!([
                {"commit": "bbbbbbb", "message": "address review", "age_ms": 10 * 3_600_000,
                 "waits_on": "service", "service_files": ["src/server.rs"], "shared_files": [],
                 "unread": null},
                {"commit": "ddddddd", "message": "a flag for every command",
                 "age_ms": 8 * 3_600_000, "waits_on": "can't tell", "service_files": [],
                 "shared_files": ["src/web.rs"], "unread": null},
            ])
        );
        assert_eq!(
            (&merged["count"], &merged["cant_tell"]),
            (&json!(1), &json!(1))
        );
        assert_eq!(merged["oldest_age_ms"], 10 * 3_600_000);
        assert_eq!(
            merged["summary"],
            "1 commit naming no story merged, not live on the service; the oldest for 10 h; 1 commit naming no story can't be told from the paths (shared with the service: src/web.rs): read the diff; 1 story changes only the CLI, the viewer, docs or tests: live wherever those builds have it (see cli_build and viewer_build)"
        );
        assert!(out["hint"].is_string(), "a commit waits on the service");

        // Only docs in the unnamed commits: nothing to report, and no hint.
        let calm = section(input(files(&[
            ("a", &["docs/watchman.md"]),
            ("b", &["docs/brief.md"]),
            ("c", &["docs/brief.md"]),
            ("d", &["tests/cli.rs"]),
        ])));
        assert_eq!(calm["merged_not_live"]["unnamed"], json!([]));
        assert_eq!(calm["merged_not_live"]["count"], 0);
        assert!(calm.get("hint").is_none());

        // An unnamed commit whose files weren't read can't be told.
        let partial = section(input(files(&[
            ("a", &["docs/watchman.md"]),
            ("c", &["docs/brief.md"]),
            ("d", &["tests/cli.rs"]),
        ])));
        assert_eq!(
            partial["merged_not_live"]["unnamed"][0]["waits_on"],
            "can't tell"
        );
        assert_eq!(
            partial["merged_not_live"]["unnamed"][0]["unread"],
            "bbbbbbb's files weren't read"
        );
        assert!(partial["hint"].is_string());
    }

    /// RAL-195: what's read is each commit on main's own line, whose files (against its first
    /// parent) are all its pull request changed, whatever the pull request's commits are called.
    /// A merge of main into a branch isn't read: it repeats what main already had.
    #[test]
    fn what_landed_on_main_is_each_commit_on_its_own_line() {
        let commit = |sha: &str, message: &str, parents: &[&str]| {
            json!({"sha": sha, "message": message, "date": "2026-11-15T01:00:00Z",
                   "parents": parents})
        };
        // live - m1 - m2 - m3 - d, where m1 merges a named branch with an unnamed commit, m2 a
        // branch that names no story and merged main in, m3 a branch whose merge names it, and d
        // is a commit made straight on main.
        let compare = json!({"status": "ahead", "ahead_by": 9, "commits": [
            commit("a1", "RAL-190: a browser test", &["live"]),
            commit("a2", "address review", &["a1"]),
            commit("m1", "Merge pull request #277 from o/ral-ages-flake", &["live", "a2"]),
            commit("b1", "tidy", &["live"]),
            commit("b2", "Merge remote-tracking branch 'origin/main' into tidy", &["b1", "m1"]),
            commit("m2", "Merge pull request #278 from o/tidy", &["m1", "b2"]),
            commit("c1", "wip", &["m2"]),
            commit("m3", "Merge pull request #279 from o/ral-192", &["m2", "c1"]),
            commit("d", "a fix made on main", &["m3"]),
        ]});
        let landed = landings(&compare);
        let brief: Vec<(&str, Vec<&str>)> = landed
            .iter()
            .map(|l| {
                (
                    l.sha.as_str(),
                    l.stories.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            brief,
            [
                ("m1", vec!["RAL-190"]),
                ("m2", vec![]),
                ("m3", vec!["RAL-192"]),
                ("d", vec![]),
            ]
        );
        assert_eq!(
            commits_to_read(&compare, 60).unwrap(),
            ["m1", "m2", "m3", "d"]
        );
        // The unnamed "address review" changed the service: its pull request's merge says so,
        // and RAL-190 waits on the service.
        let read: CommitFiles = [
            (
                "m1",
                json!([{"filename": "tests/web.rs"}, {"filename": "src/server.rs"}]),
            ),
            ("m2", json!([{"filename": "src/store.rs"}])),
            ("m3", json!([{"filename": "src/lock_holder.rs"}])),
            ("d", json!([{"filename": "docs/brief.md"}])),
        ]
        .into_iter()
        .map(|(sha, files)| (sha.to_owned(), Ok(files)))
        .collect();
        let out = section(Read {
            live: Some("live"),
            main: Some(Ok("d".into())),
            behind: Some(Ok(compare.clone())),
            files: Some(Ok(read)),
            now_ms: iso_ms("2026-11-15T03:00:00Z").unwrap(),
            ..Read::default()
        });
        let merged = &out["merged_not_live"];
        assert_eq!(merged["stories"][0]["story"], "RAL-190");
        assert_eq!(
            merged["stories"][0]["service_files"],
            json!(["src/server.rs"])
        );
        assert_eq!(merged["stories"][1]["waits_on"], "cli or viewer");
        assert_eq!(merged["unnamed"][0]["commit"], "m2");
        assert_eq!(
            merged["unnamed"][0]["service_files"],
            json!(["src/store.rs"])
        );
        assert_eq!(
            merged["summary"],
            "1 story and 1 commit naming no story merged, not live on the service; the oldest for 2 h; 1 story changes only the CLI, the viewer, docs or tests: live wherever those builds have it (see cli_build and viewer_build)"
        );

        // More landed than are read, or fewer listed than merged: nothing is read, and it's said.
        assert_eq!(
            commits_to_read(&compare, 3).unwrap_err(),
            "4 commits landed on main, more than the 3 whose files are read"
        );
        let short = json!({"ahead_by": 300, "commits": compare["commits"]});
        assert_eq!(
            commits_to_read(&short, 60).unwrap_err(),
            "GitHub lists only 9 of the 300 commits, so their files can't say everything that changed"
        );
    }

    #[test]
    fn a_path_belongs_to_a_part() {
        assert_eq!(part_of("src/server.rs"), Part::Service);
        assert_eq!(part_of("src/views.rs"), Part::Service);
        assert_eq!(part_of("Cargo.lock"), Part::Service);
        assert_eq!(part_of("src/main.rs"), Part::Client);
        assert_eq!(part_of("src/serve.rs"), Part::Service);
        assert_eq!(part_of("src/web.rs"), Part::Shared);
        assert_eq!(part_of("src/a_module_added_since.rs"), Part::Shared);
        assert_eq!(part_of("src/session.rs"), Part::Client);
        assert_eq!(part_of("web/app.js"), Part::Client);
        assert_eq!(part_of("tests/service.rs"), Part::Neither);
        assert_eq!(part_of("docs/roadmap.md"), Part::Neither);
        // lib.rs: naming a client module is the client's; anything else is shared.
        let lib = |patch: &str| part_of_file(&json!({"filename": "src/lib.rs", "patch": patch}));
        assert_eq!(
            lib("@@ -1 +1,2 @@\n pub mod shims;\n+pub mod slot_queue;"),
            Part::Client
        );
        assert_eq!(
            lib("@@ -1 +1,2 @@\n pub mod shims;\n+pub mod store;"),
            Part::Shared
        );
        assert_eq!(lib("@@ -1 +1,2 @@\n+pub mod brand_new;"), Part::Shared);
        assert_eq!(
            lib("@@ -1 +1,2 @@\n+pub const COMMIT: &str = \"\";"),
            Part::Shared
        );
        assert_eq!(
            part_of_file(&json!({"filename": "src/lib.rs"})),
            Part::Shared
        );
    }

    /// RAL-196: `main` is the client's because all it does for the service is pass `serve` on.
    /// If serve's wiring comes back into it, a change to it could be the service's again.
    #[test]
    fn main_only_passes_serve_on() {
        let main = include_str!("main.rs");
        assert!(
            main.contains("Command::Serve(args) => return rally::serve::run(location, args),"),
            "main passes `serve` on whole"
        );
        assert!(main.contains("    Serve(rally::serve::Args),"));
        // The flags that name the ledger are the store's, flattened in: main declares none.
        assert!(main.contains("    ledger: rally::store::LedgerFlags,"));
        assert!(main.contains("cli.ledger.location(uses_database)?"));
        for services in [
            "server::",
            "RateLimit",
            "evidence::Settings",
            "RALLY_DB",
            "RALLY_HEAD_FILE",
            "cli.db",
            "cli.head_file",
            " head_file: ",
            "Location {",
        ] {
            assert!(
                !main.contains(services),
                "src/main.rs names {services}, which is the service's: keep it in src/serve.rs, \
                 or move `main` back to builds::SHARED_MODULES"
            );
        }
    }

    /// Every file in `src/` is placed in one list, so a new module makes its author say whether
    /// the service runs it; and the service's list holds what `server` and `store` name.
    #[test]
    fn every_module_is_placed() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let lists = [SERVICE_MODULES, SHARED_MODULES, CLIENT_MODULES];
        for entry in std::fs::read_dir(&src).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            let module = name.trim_end_matches(".rs");
            let placed = lists.iter().filter(|list| list.contains(&module)).count();
            assert_eq!(
                placed, 1,
                "src/{name} must be in exactly one of builds::SERVICE_MODULES (the service runs \
                 it), SHARED_MODULES or CLIENT_MODULES"
            );
        }
        // What the service's own modules use outside their tests is the service's or shared.
        for module in SERVICE_MODULES {
            let text = std::fs::read_to_string(src.join(format!("{module}.rs"))).unwrap();
            let code = text.split("\n#[cfg(test)]\nmod tests").next().unwrap();
            for (at, _) in code.match_indices("crate::") {
                let used: String = code[at + 7..]
                    .chars()
                    .take_while(|c| c.is_ascii_lowercase() || *c == '_')
                    .collect();
                assert!(
                    !CLIENT_MODULES.contains(&used.as_str()),
                    "src/{module}.rs uses crate::{used}, so the service runs it: move it to \
                     builds::SERVICE_MODULES"
                );
            }
        }
    }
}
