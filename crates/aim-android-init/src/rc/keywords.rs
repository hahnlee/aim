//! init's `KeywordMap` (system/core/init/keyword_map.h): every builtin and
//! service option has an argument-count range checked at parse time.

/// `std::numeric_limits<size_t>::max()` in the upstream tables.
pub(crate) const UNBOUNDED: usize = usize::MAX;

/// `KeywordMap::Find` for a table of `(keyword, min_args, max_args, value)`.
/// `args[0]` is the keyword; the count excludes it.
pub(crate) fn find<'a, T>(
    table: &'a [(&'static str, usize, usize, T)],
    args: &[String],
) -> Result<&'a T, String> {
    let Some(keyword) = args.first() else {
        return Err("Keyword needed, but not provided".to_string());
    };
    let num_args = args.len() - 1;
    let Some((_, min_args, max_args, value)) =
        table.iter().find(|(name, ..)| *name == keyword.as_str())
    else {
        return Err(format!("Invalid keyword '{keyword}'"));
    };
    let (min_args, max_args) = (*min_args, *max_args);
    if min_args == max_args && num_args != min_args {
        let plural = if min_args > 1 || min_args == 0 {
            "s"
        } else {
            ""
        };
        return Err(format!("{keyword} requires {min_args} argument{plural}"));
    }
    if num_args < min_args || num_args > max_args {
        if max_args == UNBOUNDED {
            let plural = if min_args > 1 { "s" } else { "" };
            return Err(format!(
                "{keyword} requires at least {min_args} argument{plural}"
            ));
        }
        return Err(format!(
            "{keyword} requires between {min_args} and {max_args} arguments"
        ));
    }
    Ok(value)
}
