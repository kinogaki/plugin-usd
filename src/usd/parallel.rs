//! Independent jobs on the process's workers, the SDK's (on the calling
//! thread alone where no host installed any): large arrays parse, format,
//! compress and decode in chunks within the host's one budget, never on
//! threads of their own. Results never depend on how many threads a run got.

use kinogaki_studio_api::geometry::parallel;
use std::convert::Infallible;

/// `job(index)` for every index below `count`, the results in index order.
#[track_caller]
pub fn map<R: Send>(count: usize, job: impl Fn(usize) -> R + Sync) -> Vec<R> {
    match parallel::map_blocks(count, 1, |range| Ok::<_, Infallible>(job(range.start))) {
        Ok(results) => results,
        Err(never) => match never {},
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn map_keeps_order() {
        let squares = super::map(100, |index| index * index);
        assert_eq!(squares[7], 49);
        assert_eq!(squares.len(), 100);
    }
}
