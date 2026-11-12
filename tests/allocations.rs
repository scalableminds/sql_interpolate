//! Counts heap allocations while building tokens. Its own test binary, because it installs a global allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use sql_interpolate::{IntoSqlToken, SqlToken, q};

struct CountingAllocator;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn allocations<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let before = ALLOCATIONS.with(Cell::get);
    let result = f();
    (result, ALLOCATIONS.with(Cell::get) - before)
}

fn fragment(id: &String) -> SqlToken<'_> {
    q!("(owner = ${id} OR ${id} IS NULL)")
}

#[test]
fn borrowed_and_scalar_values_are_not_allocated() {
    let name = String::from("bob");
    let organization: &str = "org";
    let flag: Option<bool> = None;

    // Only the token's list of parts is allocated
    let (token, count) = allocations(|| {
        q!(
            "SELECT * FROM users WHERE name = ${&name} AND organization = ${organization}
            AND id > ${5i64} AND admin = ${true} AND active IS NOT DISTINCT FROM ${flag}"
        )
    });
    assert_eq!(count, 1);
    assert_eq!(IntoSqlToken::part_count(&token), 10);
}

#[test]
fn spliced_tokens_reallocate_at_most_once() {
    let id = String::from("abc");
    // the fragment's list, the query's list and one reallocation for splicing
    let (_, count) =
        allocations(|| q!("SELECT * FROM t WHERE ${fragment(&id)} AND x = ${1i32} LIMIT ${10i64}"));
    assert_eq!(count, 3);
}
