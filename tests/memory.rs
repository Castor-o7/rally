//! Memory kept per append, counted by the allocator rather than guessed from the process size.
//!
//! With no listener, myko's saga channel is never drained, so anything sent to it stays forever.
//! Two copies of each record are expected: myko's store holds its text, and the state the service
//! keeps between requests holds it parsed (RAL-35). Nothing else should grow with every append.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicI64, Ordering};

use rally::record::{Kind, Record, Source};
use rally::scratch_pg;
use rally::store::{Location, Store};
use serde_json::json;

struct Counting;

static LIVE: AtomicI64 = AtomicI64::new(0);

// SAFETY: forwards to the system allocator, only counting.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as i64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as i64, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(new_size as i64 - layout.size() as i64, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[test]
fn an_append_keeps_no_more_than_the_store_and_the_kept_state() {
    let db = scratch_pg::shared().database();
    let dir = tempfile::tempdir().unwrap();
    let location = Location {
        url: db.url.clone(),
        head_file: dir.path().join("ledger.head"),
        password_file: None,
    };
    let store = Store::open(&location).unwrap();
    let note: String = (0..4000u32)
        .map(|i| char::from_u32(0x4e00 + (i * 7919) % 20000).unwrap())
        .collect();
    let record = |i: u64| {
        let source = Source {
            credential: "a1".into(),
            declared_name: Some("x".into()),
        };
        let body = json!({
            "change": "team_state", "team": format!("t{}", i % 5), "status": "blocked", "note": note
        });
        Record::new(Kind::Declared, source, 1, body).unwrap()
    };
    let size = record(0).to_json().len() as i64;
    for i in 0..20 {
        store.append(&record(i)).unwrap();
    }
    let before = LIVE.load(Ordering::Relaxed);
    let count = 200;
    for i in 20..20 + count {
        store.append(&record(i)).unwrap();
    }
    let kept = (LIVE.load(Ordering::Relaxed) - before) / count as i64;
    eprintln!("each append kept {kept} bytes for a {size}-byte record");
    // myko's store holds the record's text once, with its hash and id, and the kept state holds
    // its note once more. A third copy anywhere, such as an event on an undrained channel, adds
    // at least another record's size.
    assert!(
        kept < size * 5 / 2,
        "each append kept {kept} bytes for a {size}-byte record"
    );
}
