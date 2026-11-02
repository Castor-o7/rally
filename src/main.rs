//! Rally: a ledger of progress for a muster of agents. Not built yet; see docs/roadmap.md.

const NOT_BUILT: &str = "rally has no features yet; see docs/roadmap.md";

fn main() {
    println!("{NOT_BUILT}");
}

#[cfg(test)]
mod tests {
    use super::NOT_BUILT;

    #[test]
    fn says_it_is_not_built() {
        assert!(NOT_BUILT.contains("no features"));
    }
}
