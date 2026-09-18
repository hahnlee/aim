use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const MAGIC: &[u8] = b"DARWIN_ART_NATIVE_LINK_RECIPE";
const VERSION: &[u8] = b"1";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkIdentity {
    pub(crate) output: PathBuf,
    pub(crate) map: PathBuf,
    pub(crate) install_name: OsString,
}

#[derive(Debug)]
struct ValidatedCommand {
    program: OsString,
    args: Vec<OsString>,
    identity: LinkIdentity,
    inputs: Vec<PathBuf>,
}

pub(crate) fn write_successful_recipe(path: &Path, command: &Command) -> crate::Result<()> {
    let validated = validate_command(command)?;
    let mut encoded = Vec::new();
    push_field(&mut encoded, MAGIC)?;
    push_field(&mut encoded, VERSION)?;
    push_field(&mut encoded, validated.program.as_bytes())?;
    for arg in &validated.args {
        push_field(&mut encoded, arg.as_bytes())?;
    }
    atomic_write(path, &encoded)
}

pub(crate) fn transform_recipe(
    recipe_path: &Path,
    expected: &LinkIdentity,
    test: &LinkIdentity,
    test_object: &Path,
    test_export: &OsStr,
) -> crate::Result<Command> {
    let command = read_recipe(recipe_path)?;
    let validated = validate_command(&command)?;
    if &validated.identity != expected {
        return Err(invalid(
            "recipe link identity does not match expected product",
        ));
    }
    validate_test_identity(expected, test, test_object, test_export, &validated.inputs)?;

    let mut transformed = Command::new(&validated.program);
    let mut index = 0;
    while index < validated.args.len() {
        let arg = &validated.args[index];
        if arg == OsStr::new("-o") {
            transformed.arg(arg).arg(&test.output);
            index += 2;
            continue;
        }
        if linker_value(arg, b"-Wl,-map,").is_some() {
            transformed.arg(OsString::from_vec(
                [b"-Wl,-map,".as_slice(), test.map.as_os_str().as_bytes()].concat(),
            ));
            index += 1;
            continue;
        }
        if linker_value(arg, b"-Wl,-install_name,").is_some() {
            transformed.arg(OsString::from_vec(
                [
                    b"-Wl,-install_name,".as_slice(),
                    test.install_name.as_bytes(),
                ]
                .concat(),
            ));
            index += 1;
            continue;
        }
        transformed.arg(arg);
        index += 1;
    }
    transformed.arg(test_object);
    transformed.arg(OsString::from_vec(
        [b"-Wl,-exported_symbol,".as_slice(), test_export.as_bytes()].concat(),
    ));
    Ok(transformed)
}

fn read_recipe(path: &Path) -> crate::Result<Command> {
    let bytes = fs::read(path)?;
    let fields = decode_fields(&bytes)?;
    if fields.len() < 3 || fields[0] != MAGIC || fields[1] != VERSION {
        return Err(invalid("invalid native link recipe header"));
    }
    let program = OsString::from_vec(fields[2].clone());
    let mut command = Command::new(&program);
    for arg in &fields[3..] {
        command.arg(OsString::from_vec(arg.clone()));
    }
    Ok(command)
}

fn decode_fields(bytes: &[u8]) -> crate::Result<Vec<Vec<u8>>> {
    if bytes.is_empty() || *bytes.last().unwrap() != 0 {
        return Err(invalid("truncated native link recipe"));
    }
    let mut fields = bytes
        .split(|byte| *byte == 0)
        .map(|part| part.to_vec())
        .collect::<Vec<_>>();
    // split() adds one empty item for the required terminator. An empty final
    // argument remains as the preceding empty field and is therefore preserved.
    fields.pop();
    if fields
        .iter()
        .any(|field| field.iter().any(|byte| *byte == 0))
    {
        return Err(invalid("invalid native link recipe field"));
    }
    Ok(fields)
}

fn validate_command(command: &Command) -> crate::Result<ValidatedCommand> {
    if command.get_current_dir().is_some()
        || command.get_envs().any(|(_, value)| value.is_some())
        || command.get_envs().any(|(_, value)| value.is_none())
    {
        return Err(invalid(
            "link recipe does not support explicit environment or current directory",
        ));
    }
    let program = command.get_program().to_os_string();
    if program != OsStr::new("clang++") {
        return Err(invalid("native link recipe compiler must be clang++"));
    }
    let args = command
        .get_args()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let mut output = None;
    let mut map = None;
    let mut install_name = None;
    let mut dynamiclib_count = 0;
    let mut inputs = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg.as_bytes().first() == Some(&b'@') {
            return Err(invalid("response-file arguments are not supported"));
        }
        if arg == OsStr::new("-dynamiclib") {
            dynamiclib_count += 1;
        } else if arg == OsStr::new("-o") {
            if output.is_some() || index + 1 >= args.len() || args[index + 1].is_empty() {
                return Err(invalid("link recipe requires exactly one -o value"));
            }
            output = Some(PathBuf::from(&args[index + 1]));
            index += 1;
        } else if let Some(value) = linker_value(arg, b"-Wl,-map,") {
            if map.is_some() || value.is_empty() {
                return Err(invalid("link recipe requires exactly one map value"));
            }
            map = Some(PathBuf::from(OsString::from_vec(value.to_vec())));
        } else if let Some(value) = linker_value(arg, b"-Wl,-install_name,") {
            if install_name.is_some() || value.is_empty() {
                return Err(invalid(
                    "link recipe requires exactly one install_name value",
                ));
            }
            install_name = Some(OsString::from_vec(value.to_vec()));
        } else if let Some(value) = linker_value(arg, b"-Wl,-force_load,") {
            require_input_file(value, "force_load archive")?;
            inputs.push(PathBuf::from(OsString::from_vec(value.to_vec())));
        } else if let Some(value) = linker_value(arg, b"-Wl,-exported_symbols_list,") {
            require_input_file(value, "export list")?;
            inputs.push(PathBuf::from(OsString::from_vec(value.to_vec())));
        } else if arg == OsStr::new("-o=")
            || arg.as_bytes().starts_with(b"-o=")
            || arg.as_bytes().starts_with(b"-o/")
            || arg.as_bytes().starts_with(b"-Wl,-o,")
            || arg.as_bytes().starts_with(b"-Wl,-map=")
            || arg.as_bytes().starts_with(b"-Wl,-install_name=")
            || arg == OsStr::new("-Wl,-map")
            || arg == OsStr::new("-Wl,-install_name")
        {
            return Err(invalid("unsupported linker output spelling"));
        }
        if arg == OsStr::new("-Xlinker") && index + 1 < args.len() {
            let linker_arg = &args[index + 1];
            if linker_arg == OsStr::new("-o")
                || linker_arg == OsStr::new("-map")
                || linker_arg == OsStr::new("-install_name")
            {
                return Err(invalid("unsupported -Xlinker output spelling"));
            }
            if (linker_arg == OsStr::new("-force_load")
                || linker_arg == OsStr::new("-exported_symbols_list"))
                && index + 3 < args.len()
                && args[index + 2] == OsStr::new("-Xlinker")
            {
                require_input_file(args[index + 3].as_bytes(), "linker input")?;
                inputs.push(PathBuf::from(&args[index + 3]));
                index += 3;
            }
        }
        if is_plain_input_artifact(arg) {
            require_input_file(arg.as_bytes(), "link input")?;
            inputs.push(PathBuf::from(arg));
        }
        index += 1;
    }
    if dynamiclib_count != 1 {
        return Err(invalid("link recipe requires exactly one -dynamiclib"));
    }
    let identity = LinkIdentity {
        output: output.ok_or_else(|| invalid("missing product output"))?,
        map: map.ok_or_else(|| invalid("missing product map"))?,
        install_name: install_name.ok_or_else(|| invalid("missing product install_name"))?,
    };
    if paths_alias(&identity.output, &identity.map) {
        return Err(invalid("product output aliases product map"));
    }
    Ok(ValidatedCommand {
        program,
        args,
        identity,
        inputs,
    })
}

fn validate_test_identity(
    expected: &LinkIdentity,
    test: &LinkIdentity,
    test_object: &Path,
    test_export: &OsStr,
    inputs: &[PathBuf],
) -> crate::Result<()> {
    if test.output.as_os_str().is_empty()
        || test.map.as_os_str().is_empty()
        || test.install_name.is_empty()
        || paths_alias(&test.output, &expected.output)
        || paths_alias(&test.map, &expected.map)
        || paths_alias(&test.output, &expected.map)
        || paths_alias(&test.map, &expected.output)
        || paths_alias(&test.output, test_object)
        || paths_alias(&test.map, test_object)
        || test.install_name == expected.install_name
        || paths_alias(&test.output, &test.map)
        || test_export.is_empty()
        || test_export.as_bytes().contains(&0)
        || !test_export.as_bytes().starts_with(b"_")
        || !test_export
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        return Err(invalid("test link identity aliases product or is empty"));
    }
    if !test_object.is_file() {
        return Err(invalid("test object is missing"));
    }
    normalized_path(&test.output)?;
    normalized_path(&test.map)?;
    for input in inputs {
        if paths_alias(input, &test.output) || paths_alias(input, &test.map) {
            return Err(invalid("test output aliases an existing link artifact"));
        }
    }
    Ok(())
}

fn linker_value<'a>(arg: &'a OsStr, prefix: &[u8]) -> Option<&'a [u8]> {
    arg.as_bytes().strip_prefix(prefix)
}

fn is_plain_input_artifact(arg: &OsStr) -> bool {
    if arg.is_empty()
        || arg.as_bytes().first() == Some(&b'-')
        || arg.as_bytes().first() == Some(&b'@')
    {
        return false;
    }
    [
        b".o".as_slice(),
        b".a".as_slice(),
        b".dylib".as_slice(),
        b".bundle".as_slice(),
        b".tbd".as_slice(),
        b".so".as_slice(),
    ]
    .iter()
    .any(|suffix| arg.as_bytes().ends_with(suffix))
}

fn require_input_file(bytes: &[u8], kind: &str) -> crate::Result<()> {
    let path = PathBuf::from(OsString::from_vec(bytes.to_vec()));
    if path.as_os_str().is_empty() || !path.is_file() {
        return Err(invalid(&format!("{kind} is missing")));
    }
    Ok(())
}

fn paths_alias(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (normalized_path(left), normalized_path(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn normalized_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if fs::symlink_metadata(&absolute).is_ok() {
        return fs::canonicalize(absolute);
    }
    let file = absolute
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no filename"))?;
    let parent = absolute.parent().unwrap_or_else(|| Path::new("."));
    Ok(fs::canonicalize(parent)?.join(file))
}

fn push_field(output: &mut Vec<u8>, field: &[u8]) -> crate::Result<()> {
    if field.contains(&0) {
        return Err(invalid("NUL bytes are not valid command fields"));
    }
    output.extend_from_slice(field);
    output.push(0);
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> crate::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| invalid("recipe path has no filename"))?;
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let mut temporary = parent.join(OsString::from_vec(
        [
            b".native-link-recipe.".as_slice(),
            name.as_bytes(),
            b".".as_slice(),
            nonce.to_string().as_bytes(),
        ]
        .concat(),
    ));
    for attempt in 0..16u32 {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(mut file) => {
                let result = (|| -> io::Result<()> {
                    file.write_all(bytes)?;
                    file.sync_all()?;
                    Ok(())
                })();
                drop(file);
                if let Err(error) = result {
                    let _ = fs::remove_file(&temporary);
                    return Err(error.into());
                }
                fs::rename(&temporary, path)?;
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                temporary = parent.join(format!(".native-link-recipe.{}.{}", nonce, attempt));
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(invalid("could not allocate temporary recipe path"))
}

fn invalid(message: &str) -> Box<dyn std::error::Error> {
    io::Error::new(io::ErrorKind::InvalidData, message).into()
}

#[cfg(test)]
#[path = "../../../tools/tests/native-link-recipe.rs"]
mod tests;
