//! Shared exact-ref porcelain decoding. Callers apply their own rewrite policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Created,
    Deleted,
    Advanced,
    Unchanged,
    Rewritten,
    Rejected,
    Unconfirmed,
}
#[derive(Debug)]
pub(crate) struct Response {
    pub destination: Vec<u8>,
    pub state: Status,
    pub detail: Vec<u8>,
}

pub(crate) fn responses(output: &[u8], source: &[u8], target: &[u8]) -> (Vec<Response>, bool) {
    let expected_refspec = [source, b":".as_slice(), target].concat();
    let mut outcomes = vec![];
    let mut destination = None;
    let mut valid = true;
    let mut seen = false;
    for line in output.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        if let Some(url) = line.strip_prefix(b"To ") {
            if destination.is_some() {
                valid = false;
            }
            destination = Some(url.to_vec());
            seen = false;
        } else if line == b"Done" && destination.is_some() {
            if !seen {
                valid = false;
            }
            destination = None;
        } else {
            let fields: Vec<_> = line.split(|b| *b == b'\t').collect();
            // Normal hooks can print progress on stdout. Only ref-status-shaped
            // lines participate in protocol validation; preserve all other text
            // in the raw process output without discarding acknowledgements.
            if fields.len() < 2 || fields[0].len() != 1 || !b" *!=+-".contains(&fields[0][0]) {
                continue;
            }
            if fields.len() != 3 || fields[1] != expected_refspec || destination.is_none() || seen {
                valid = false;
                continue;
            }
            let state = match fields[0][0] {
                b'*' => Status::Created,
                b'-' if source.is_empty() => Status::Deleted,
                b' ' => Status::Advanced,
                b'=' => Status::Unchanged,
                b'+' => Status::Rewritten,
                b'!' if fields[2].starts_with(b"[rejected]")
                    || fields[2].starts_with(b"[remote rejected]") =>
                {
                    Status::Rejected
                }
                _ => Status::Unconfirmed,
            };
            outcomes.push(Response {
                destination: destination.clone().unwrap(),
                state,
                detail: fields[2].to_vec(),
            });
            seen = true;
        }
    }
    // A missing Done does not erase an explicit per-ref acknowledgement.
    if destination.is_some() && !seen {
        valid = false;
    }
    (outcomes, valid)
}
