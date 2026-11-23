use std::fmt::Debug;

use anyhow::{Result, bail};

/// Each contract, with its name.
macro_rules! contracts {
    ($($contract:ident),* $(,)?) => {
        pub fn contracts() -> Vec<(&'static str, $crate::sandbox::Contract)> {
            use futures_util::FutureExt as _;
            vec![$((stringify!($contract), |sandbox| $contract(sandbox).boxed())),*]
        }
    };
}

pub(crate) use contracts;

/// Fails, saying `what` differs and how, unless `actual` is `expected`.
/// Never panics.
pub fn equal<T: PartialEq + Debug + ?Sized>(what: &str, actual: &T, expected: &T) -> Result<()> {
    if actual != expected {
        bail!("{what} is {actual:?}, not {expected:?}");
    }
    Ok(())
}
