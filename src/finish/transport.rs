//! Command-scoped exact endpoints, shared by distinct workflow authorities.
use super::*;

// A command-scoped remote selects one explicit push URL and bypasses a second
// pushInsteadOf expansion. Copy configured transport options (receivepack,
// proxy, vcs, etc.), but no selection defaults or tracking mappings. No remote
// is persisted. Verify read and push resolution before contacting the endpoint.
pub(super) fn endpoint(
    config: &[u8],
    remote: &str,
    destination: &[u8],
) -> Result<(Vec<OsString>, String), FinishError> {
    let entries: Vec<_> = config
        .split(|b| *b == 0)
        .filter(|e| !e.is_empty())
        .map(|e| {
            e.iter()
                .position(|b| *b == b'\n')
                .map_or((e, None), |i| (&e[..i], Some(&e[i + 1..])))
        })
        .collect();
    let mut name = "oneflow-finish-publication".to_owned();
    while entries
        .iter()
        .any(|(key, _)| key.starts_with(format!("remote.{name}.").as_bytes()))
    {
        name.push('-');
    }
    let prefix = format!("remote.{}.", remote);
    let target = format!("remote.{name}.");
    let mut args = vec![];
    for (key, value) in &entries {
        if let Some(suffix) = key.strip_prefix(prefix.as_bytes()) {
            // A dot in the remainder belongs to another remote subsection.
            if suffix.contains(&b'.')
                || [b"url".as_slice(), b"pushurl", b"push", b"fetch", b"mirror"].contains(&suffix)
            {
                continue;
            }
            let mut setting = [target.as_bytes(), suffix].concat();
            if let Some(value) = value {
                setting.extend_from_slice(b"=");
                setting.extend_from_slice(value);
            }
            args.extend(["-c".into(), repository::os(&setting)?]);
        }
    }
    for suffix in ["url", "pushurl"] {
        args.extend([
            "-c".into(),
            repository::os(&[format!("{target}{suffix}=").as_bytes(), destination].concat())?,
        ]);
    }
    args.extend([
        "-c".into(),
        format!("{target}mirror=false").into(),
        "-c".into(),
        "push.autoSetupRemote=false".into(),
    ]);
    Ok((args, name))
}
