//! Optional native embedding guard. Ordinary reference parsing is unlimited.
use std::cell::Cell;
#[derive(Clone, Copy, Default)]
struct Budget {
    limit: usize,
    depth: usize,
    exceeded: bool,
}
thread_local! {static CURRENT:Cell<Budget>=Cell::new(Budget::default());}
pub struct ParseRecursionLimit;
pub fn with_parse_recursion_limit<T>(
    limit: usize,
    operation: impl FnOnce() -> T,
) -> Result<T, ParseRecursionLimit> {
    struct Restore(Budget);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(CURRENT.with(|c| c.replace(Budget { limit, ..Default::default() })));
    let result = operation();
    if CURRENT.with(|c| c.get().exceeded) { Err(ParseRecursionLimit) } else { Ok(result) }
}
pub struct Depth(bool);
impl Drop for Depth {
    fn drop(&mut self) {
        if self.0 {
            CURRENT.with(|c| {
                let mut b = c.get();
                b.depth -= 1;
                c.set(b);
            });
        }
    }
}
pub fn enter() -> Option<Depth> {
    CURRENT.with(|c| {
        let mut b = c.get();
        if b.limit == 0 {
            return Some(Depth(false));
        }
        if b.exceeded || b.depth >= b.limit {
            b.exceeded = true;
            c.set(b);
            return None;
        }
        b.depth += 1;
        c.set(b);
        Some(Depth(true))
    })
}
