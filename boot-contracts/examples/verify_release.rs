use boot_contracts::release::{INITIAL_TRUST_ROOT, apply_rotation, verify_ed25519};

fn decode_hex<const N: usize>(value: &str) -> Result<[u8; N], &'static str> {
    if value.len() != N * 2 {
        return Err("bad hex length");
    }
    let mut output = [0u8; N];
    for (index, byte) in output.iter_mut().enumerate() {
        let offset = index * 2;
        *byte = u8::from_str_radix(&value[offset..offset + 2], 16).map_err(|_| "bad hex")?;
    }
    Ok(output)
}

fn main() -> Result<(), &'static str> {
    let mut arguments = std::env::args().skip(1);
    let mode = arguments.next().ok_or("missing mode")?;
    match mode.as_str() {
        "signature" => {
            let public_key = decode_hex::<32>(&arguments.next().ok_or("missing public key")?)?;
            let payload = decode_hex::<73>(&arguments.next().ok_or("missing payload")?)?;
            let signature = decode_hex::<64>(&arguments.next().ok_or("missing signature")?)?;
            if arguments.next().is_some() {
                return Err("too many arguments");
            }
            verify_ed25519(&public_key, &payload, &signature)
                .map_err(|_| "signature verification failed")
        }
        "rotation" => {
            let path = arguments.next().ok_or("missing rotation path")?;
            if arguments.next().is_some() {
                return Err("too many arguments");
            }
            let bytes = std::fs::read(path).map_err(|_| "rotation read failed")?;
            apply_rotation(&INITIAL_TRUST_ROOT, &bytes)
                .map(|_| ())
                .map_err(|_| "rotation verification failed")
        }
        _ => Err("unknown mode"),
    }
}
