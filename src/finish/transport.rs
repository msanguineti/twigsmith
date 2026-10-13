//! Command-scoped exact endpoints, shared by distinct workflow authorities.
use super::*;

pub(super) fn endpoint(
    config: &[u8],
    remote: &str,
    destination: &[u8],
) -> Result<(Vec<OsString>, String), FinishError> {
    Ok(crate::publication::transport::endpoint(
        config,
        remote,
        destination,
    )?)
}
