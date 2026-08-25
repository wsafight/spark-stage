use super::*;

pub(super) fn execute_edit(args: EditArgs) -> Result<ExitCode, CliError> {
    let (project, connection, command, json, mutating) = match args.command {
        EditCommand::Build {
            project,
            kind,
            shots,
            connection,
            json,
        } => {
            if shots.is_some() && kind != "draft" {
                return Err(CliError::InvalidInput(
                    "--shots is only valid with --kind draft".to_owned(),
                ));
            }
            let shot_ids = shots
                .as_deref()
                .map(expand_shot_selection)
                .transpose()
                .map_err(CliError::InvalidInput)?
                .unwrap_or_default();
            (
                project,
                connection,
                IpcCommand::Build { kind, shot_ids },
                json,
                true,
            )
        }
        EditCommand::Trailer {
            project,
            connection,
            json,
        } => (
            project,
            connection,
            IpcCommand::Build {
                kind: "trailer".to_owned(),
                shot_ids: Vec::new(),
            },
            json,
            true,
        ),
        EditCommand::Open {
            project,
            build,
            connection,
            json,
        } => (
            project,
            connection,
            IpcCommand::OpenBuild { build_id: build },
            json,
            false,
        ),
    };
    let client = WorkerClient::new(resolved_paths(&connection).socket, Some(project));
    let revision = if mutating {
        Some(current_revision(&client)?)
    } else {
        None
    };
    let reply = client.send(command, revision)?;
    print_reply(&reply, json)?;
    Ok(reply_exit_code(&reply))
}

pub(super) fn expand_shot_selection(value: &str) -> Result<Vec<String>, String> {
    const MAX_EXPANDED_SHOTS: usize = 1_000;

    let mut result = Vec::new();
    for raw_segment in value.split(',') {
        let segment = raw_segment.trim();
        if segment.is_empty() {
            return Err("shot selection contains an empty item".to_owned());
        }
        let Some((start, end)) = segment.split_once('-') else {
            if result.len() >= MAX_EXPANDED_SHOTS {
                return Err(format!(
                    "shot selection expands beyond {MAX_EXPANDED_SHOTS} items"
                ));
            }
            result.push(segment.to_owned());
            continue;
        };
        if end.contains('-') {
            return Err(format!("invalid shot range `{segment}`"));
        }
        let (start_prefix, start_number, start_width) = split_numeric_suffix(start)
            .ok_or_else(|| format!("range start `{start}` must end in digits"))?;
        let (end_prefix, end_number, end_width) = split_numeric_suffix(end)
            .ok_or_else(|| format!("range end `{end}` must end in digits"))?;
        if start_prefix != end_prefix || start_number > end_number {
            return Err(format!("invalid ascending shot range `{segment}`"));
        }
        let width = start_width.max(end_width);
        for number in start_number..=end_number {
            if result.len() >= MAX_EXPANDED_SHOTS {
                return Err(format!(
                    "shot selection expands beyond {MAX_EXPANDED_SHOTS} items"
                ));
            }
            result.push(format!("{start_prefix}{number:0width$}"));
        }
    }
    if result.is_empty() {
        return Err("shot selection is empty".to_owned());
    }
    Ok(result)
}

fn split_numeric_suffix(value: &str) -> Option<(&str, u32, usize)> {
    let digit_start = value.find(|character: char| character.is_ascii_digit())?;
    let (prefix, digits) = value.split_at(digit_start);
    if prefix.is_empty() || digits.is_empty() || !digits.chars().all(|value| value.is_ascii_digit())
    {
        return None;
    }
    Some((prefix, digits.parse().ok()?, digits.len()))
}
