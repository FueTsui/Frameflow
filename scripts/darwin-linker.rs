//! Translate rustc's Windows response-file quoting for LLD's Mach-O driver.
use std::{
    env,
    ffi::OsString,
    fs, io,
    path::PathBuf,
    process::{Command, ExitCode},
};

fn main() -> ExitCode {
    match link() {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(error) => {
            eprintln!("Frameflow Darwin linker: {error}");
            ExitCode::FAILURE
        }
    }
}

struct ResponseFiles(Vec<PathBuf>);
impl Drop for ResponseFiles {
    fn drop(&mut self) {
        for file in &self.0 {
            let _ = fs::remove_file(file);
        }
    }
}

fn invalid_response(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn decode_response(bytes: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        if (bytes.len() - 2) % 2 != 0 {
            return Err(
                invalid_response("response file has an incomplete UTF-16 character").into(),
            );
        }
        let little_endian = bytes[0] == 0xff;
        let words = bytes[2..]
            .chunks_exact(2)
            .map(|pair| {
                if little_endian {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            })
            .collect::<Vec<_>>();
        Ok(String::from_utf16(&words)?)
    } else {
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        Ok(std::str::from_utf8(bytes)?.to_owned())
    }
}

fn gnu_response(text: &str) -> Result<String, io::Error> {
    let mut output = String::with_capacity(text.len());
    // rustc 1.98 writes one argument per line, surrounds it with double quotes,
    // and escapes only embedded double quotes on Windows LLD invocations.
    // Decode that format before re-encoding; replacing path separators would
    // corrupt escaped quotes and non-path arguments.
    for (index, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let argument = line
            .strip_prefix('"')
            .and_then(|line| line.strip_suffix('"'))
            .ok_or_else(|| {
                invalid_response(format!(
                    "expected a quoted rustc argument on response-file line {}",
                    index + 1
                ))
            })?
            .replace("\\\"", "\"");
        output.push('"');
        for character in argument.chars() {
            if matches!(character, '\\' | '"') {
                output.push('\\');
            }
            output.push(character);
        }
        output.push_str("\"\n");
    }
    Ok(output)
}

fn link() -> Result<i32, Box<dyn std::error::Error>> {
    let linker = env::var_os("FRAMEFLOW_RUST_LLD").ok_or("FRAMEFLOW_RUST_LLD is not set")?;
    let mut temporary = ResponseFiles(Vec::new());
    let mut arguments = Vec::<OsString>::new();
    for (index, argument) in env::args_os().skip(1).enumerate() {
        if let Some(path) = argument.to_str().and_then(|text| text.strip_prefix('@')) {
            let bytes = fs::read(path)?;
            let text = gnu_response(&decode_response(&bytes)?)?;
            let file = env::temp_dir().join(format!(
                "frameflow-darwin-{}-{index}.rsp",
                std::process::id()
            ));
            temporary.0.push(file.clone());
            fs::write(&file, text.as_bytes())?;
            let mut response_argument = OsString::from("@");
            response_argument.push(file);
            arguments.push(response_argument);
        } else {
            arguments.push(argument);
        }
    }
    let result = Command::new(linker).args(arguments).status()?;
    Ok(result.code().unwrap_or(1))
}

#[cfg(test)]
mod tests {
    use super::{decode_response, gnu_response};

    #[test]
    fn preserves_windows_paths_with_spaces_cjk_and_apostrophes() {
        let input =
            "\"C:\\media folder\\中文's clip.o\"\r\n\"\\\\server\\share name\\film.rlib\"\r\n";
        let expected = "\"C:\\\\media folder\\\\中文's clip.o\"\n\"\\\\\\\\server\\\\share name\\\\film.rlib\"\n";
        assert_eq!(gnu_response(input).unwrap(), expected);
    }

    #[test]
    fn preserves_escaped_quotes_empty_arguments_and_trailing_backslashes() {
        let input = "\"symbol \\\"quoted\\\" value\"\n\"\"\n\"C:\\folder\\\"\n";
        let expected = "\"symbol \\\"quoted\\\" value\"\n\"\"\n\"C:\\\\folder\\\\\"\n";
        assert_eq!(gnu_response(input).unwrap(), expected);
    }

    #[test]
    fn reads_utf8_and_both_utf16_byte_orders() {
        let text = "\"C:\\中文 folder\\file.o\"\n";
        assert_eq!(decode_response(text.as_bytes()).unwrap(), text);
        let mut utf8_bom = vec![0xef, 0xbb, 0xbf];
        utf8_bom.extend_from_slice(text.as_bytes());
        assert_eq!(decode_response(&utf8_bom).unwrap(), text);
        for little_endian in [true, false] {
            let mut bytes = if little_endian {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for word in text.encode_utf16() {
                bytes.extend_from_slice(&if little_endian {
                    word.to_le_bytes()
                } else {
                    word.to_be_bytes()
                });
            }
            assert_eq!(decode_response(&bytes).unwrap(), text);
        }
    }

    #[test]
    fn rejects_truncated_utf16_and_unrecognized_argument_layouts() {
        assert!(decode_response(&[0xff, 0xfe, b'a']).is_err());
        assert!(gnu_response("unquoted argument\n").is_err());
    }
}
