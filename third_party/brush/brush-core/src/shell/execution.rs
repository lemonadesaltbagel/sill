//! Execution support for shell.

use std::{io::Read, path::Path};

use crate::{
    ExecutionControlFlow, ExecutionParameters, ExecutionResult, ProcessGroupPolicy, SourceInfo,
    arithmetic::Evaluatable as _, callstack, error, interp::Execute as _, openfiles,
    trace_categories,
};

impl<SE: crate::extensions::ShellExtensions> crate::Shell<SE> {
    /// Returns the default execution parameters for this shell.
    pub fn default_exec_params(&self) -> ExecutionParameters {
        let mut params = ExecutionParameters::default();

        params.process_group_policy = if self.options.enable_job_control {
            ProcessGroupPolicy::NewProcessGroup
        } else {
            ProcessGroupPolicy::SameProcessGroup
        };

        params
    }

    pub(super) async fn source_if_exists(
        &mut self,
        path: impl AsRef<Path>,
        params: &ExecutionParameters,
    ) -> Result<bool, error::Error> {
        let path = path.as_ref();
        if path.exists() {
            // TODO(startup): an `exit` in a startup file should end the shell, but
            // `load_config` has no way to tell its caller to stop; until it does, the
            // file's result is dropped. Tracked by the compat case "exit in an rc file
            // ends the shell".
            let _ = self
                .source_script(path, std::iter::empty::<String>(), params)
                .await?;
            Ok(true)
        } else {
            tracing::debug!("skipping non-existent file: {}", path.display());
            Ok(false)
        }
    }

    /// Source the given file as a shell script, returning the execution result.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to the file to source.
    /// * `args` - The arguments to pass to the script as positional parameters.
    /// * `params` - Execution parameters.
    pub async fn source_script<S: Into<String>, P: AsRef<Path>, I: Iterator<Item = S>>(
        &mut self,
        path: P,
        args: I,
        params: &ExecutionParameters,
    ) -> Result<ExecutionResult, error::Error> {
        self.parse_and_execute_script_file(
            path.as_ref(),
            args,
            params,
            callstack::ScriptCallType::Source,
        )
        .await
    }

    /// Parse and execute the given file as a shell script, returning the execution result.
    ///
    /// # Arguments
    ///
    /// * `path` - The path to the file to source.
    /// * `args` - The arguments to pass to the script as positional parameters.
    /// * `params` - Execution parameters.
    /// * `call_type` - The type of script call being made.
    async fn parse_and_execute_script_file<
        S: Into<String>,
        P: AsRef<Path>,
        I: Iterator<Item = S>,
    >(
        &mut self,
        path: P,
        args: I,
        params: &ExecutionParameters,
        call_type: callstack::ScriptCallType,
    ) -> Result<ExecutionResult, error::Error> {
        let path = path.as_ref();
        tracing::debug!("sourcing: {}", path.display());

        let mut options = std::fs::File::options();
        options.read(true);

        let opened_file: openfiles::OpenFile = self
            .open_file(&options, path, params)
            .map_err(|e| error::ErrorKind::FailedSourcingFile(path.to_owned(), e))?;

        if opened_file.is_dir() {
            return Err(error::ErrorKind::FailedSourcingFile(
                path.to_owned(),
                std::io::Error::from(std::io::ErrorKind::IsADirectory),
            )
            .into());
        }

        let source_info = crate::SourceInfo::from(path.to_owned());

        // sill: a script with Windows line endings (or a byte order mark) fails in ways that
        // don't point at the cause: bash reads each CR as part of its line, so `cd /tmp`
        // says /tmp doesn't exist and `pwd` isn't found, and an `if` never closes. Say so
        // once, up front. (Windows only: there agents and editors write such files.)
        #[cfg(windows)]
        let opened_file = {
            use std::io::Write as _;
            let mut bytes = vec![];
            std::io::Read::read_to_end(&mut { opened_file }, &mut bytes)
                .map_err(|e| error::ErrorKind::FailedSourcingFile(path.to_owned(), e))?;
            if let Some(note) = line_ending_note(&bytes, &crate::sys::fs::display_path(path)) {
                let _ = writeln!(params.stderr(self), "{note}");
            }
            std::io::Cursor::new(bytes)
        };

        let mut result = self
            .source_file(opened_file, &source_info, args, params, call_type)
            .await?;

        // sill: a sourced script's end runs the RETURN trap, as a function's does.
        if matches!(call_type, callstack::ScriptCallType::Source) {
            self.run_return_trap(Some(result.exit_code.into()), params).await;
        }

        // Handle control flow at script execution boundary. If execution completed
        // with a `return`, we need to clear it since it's already been "used". All
        // other control flow types are preserved.
        if matches!(
            result.next_control_flow,
            ExecutionControlFlow::ReturnFromFunctionOrScript
        ) {
            result.next_control_flow = ExecutionControlFlow::Normal;
        }

        Ok(result)
    }

    /// Source the given file as a shell script, returning the execution result.
    ///
    /// # Arguments
    ///
    /// * `file` - The file to source.
    /// * `source_info` - Information about the source of the script.
    /// * `args` - The arguments to pass to the script as positional parameters.
    /// * `params` - Execution parameters.
    /// * `call_type` - The type of script call being made.
    async fn source_file<F: Read, S: Into<String>, I: Iterator<Item = S>>(
        &mut self,
        file: F,
        source_info: &crate::SourceInfo,
        args: I,
        params: &ExecutionParameters,
        call_type: callstack::ScriptCallType,
    ) -> Result<ExecutionResult, error::Error> {
        let mut reader = std::io::BufReader::new(file);
        let mut parser = brush_parser::Parser::new(&mut reader, &self.parser_options());

        tracing::debug!(target: trace_categories::PARSE, "Parsing sourced file: {}", source_info.source);
        let parse_result = parser.parse_program();

        let script_positional_args = args.map(Into::into);

        self.enter_script(call_type, source_info, script_positional_args)
            .run_parsed_result(parse_result, source_info, params)
            .await
    }

    /// Executes the given string as a shell program, returning the resulting exit status.
    ///
    /// # Arguments
    ///
    /// * `command` - The command to execute.
    /// * `source_info` - Information about the source of the command text.
    /// * `params` - Execution parameters.
    pub async fn run_string<S: Into<String>>(
        &mut self,
        command: S,
        source_info: &crate::SourceInfo,
        params: &ExecutionParameters,
    ) -> Result<ExecutionResult, error::Error> {
        let parse_result = self.parse_string(command);
        self.run_parsed_result(parse_result, source_info, params)
            .await
    }

    /// Executes the given command, provided to a shell executable on the command
    /// line (i.e., via `-c`).
    ///
    /// It is expected that the shell will not be used for any further execution
    /// after this command; this function will perform any necessary shell exit
    /// handling.
    ///
    /// # Arguments
    ///
    /// * `command` - The command to execute.
    pub async fn run_dash_c_command<S: Into<String>>(
        &mut self,
        command: S,
    ) -> Result<ExecutionResult, error::Error> {
        self.start_command_string_mode();

        // Execute the command string.
        let params = self.default_exec_params();
        let source_info = SourceInfo::from("-c");
        let mut result = self.run_string(command, &source_info, &params).await?;

        self.end_command_string_mode()?;

        // Give the shell a chance to run on-exit tasks. An `exit` there sets the shell's
        // status; a failure there doesn't change the command's outcome.
        if let Ok(Some(status)) = self.run_exit_trap().await {
            result.exit_code = status.into();
        }

        Ok(result)
    }

    /// Executes the given script file, returning the resulting exit status.
    ///
    /// It is expected that the shell will not be used for any further execution
    /// after this command; this function will perform any necessary shell exit
    /// handling.
    ///
    /// # Arguments
    ///
    /// * `script_path` - The path to the script file to execute.
    /// * `args` - The arguments to pass to the script as positional parameters.
    pub async fn run_script<S: Into<String>, P: AsRef<Path>, I: Iterator<Item = S>>(
        &mut self,
        script_path: P,
        args: I,
    ) -> Result<ExecutionResult, error::Error> {
        let params = self.default_exec_params();
        let mut result = self
            .parse_and_execute_script_file(
                script_path.as_ref(),
                args,
                &params,
                callstack::ScriptCallType::Run,
            )
            .await?;

        // Give the shell a chance to run on-exit tasks. An `exit` there sets the shell's
        // status; a failure there doesn't change the script's outcome.
        if let Ok(Some(status)) = self.run_exit_trap().await {
            result.exit_code = status.into();
        }

        Ok(result)
    }

    pub(crate) async fn run_parsed_result(
        &mut self,
        parse_result: Result<brush_parser::ast::Program, brush_parser::ParseError>,
        source_info: &crate::SourceInfo,
        params: &ExecutionParameters,
    ) -> Result<ExecutionResult, error::Error> {
        // If parsing succeeded, run the program. If there's a parse error, it's fatal (per spec).
        let result = match parse_result {
            Ok(prog) => self.run_program(prog, params).await,
            Err(parse_err) => Err(error::Error::from(error::ErrorKind::ParseError(
                parse_err,
                source_info.clone(),
            ))
            .into_fatal()),
        };

        // Report any errors.
        match result {
            Ok(result) => Ok(result),
            Err(err) => {
                let _ = self.display_error(&mut params.stderr(self), &err);

                let result = err.into_result(self);
                self.set_last_exit_status(result.exit_code.into());

                Ok(result)
            }
        }
    }

    /// Executes the given parsed shell program, returning the resulting exit status.
    ///
    /// # Arguments
    ///
    /// * `program` - The program to execute.
    /// * `params` - Execution parameters.
    pub async fn run_program(
        &mut self,
        program: brush_parser::ast::Program,
        params: &ExecutionParameters,
    ) -> Result<ExecutionResult, error::Error> {
        program.execute(self, params).await
    }

    /// Evaluate the given arithmetic expression, returning the result.
    pub fn eval_arithmetic(
        &mut self,
        expr: &brush_parser::ast::ArithmeticExpr,
    ) -> Result<i64, error::Error> {
        Ok(expr.eval(self)?)
    }
}

/// sill: what's wrong with a script file's bytes for Bash, if anything: Windows (CRLF)
/// line endings, or a UTF-8 byte order mark, which bash reads as part of the first word.
#[cfg_attr(not(windows), allow(dead_code))]
fn line_ending_note(bytes: &[u8], shown: &str) -> Option<String> {
    let quoted = crate::escape::force_quote(shown, crate::escape::QuoteMode::SingleQuote);
    if bytes.starts_with(b"\xEF\xBB\xBF") {
        return Some(format!(
            "{shown}: note: the file starts with a UTF-8 byte order mark, which Bash reads as part \
             of the first command; `sed -i '1s/^\\xEF\\xBB\\xBF//' {quoted}` removes it"
        ));
    }
    bytes.windows(2).any(|w| w == b"\r\n").then(|| {
        format!(
            "{shown}: note: the file has Windows (CRLF) line endings; Bash reads each CR as part \
             of its line, so commands fail oddly (`$'pwd\\r': command not found`). \
             `sed -i 's/\\r$//' {quoted}` converts it"
        )
    })
}

#[cfg(test)]
mod line_ending_tests {
    use super::line_ending_note;

    #[test]
    fn crlf_and_bom_are_named() {
        assert!(line_ending_note(b"echo hi\n", "a.sh").is_none());
        let n = line_ending_note(b"x=1\r\ncd /tmp\r\n", "a.sh").unwrap();
        assert!(n.starts_with("a.sh: note: the file has Windows (CRLF)") && n.contains("sed -i 's/\\r$//' 'a.sh'"), "{n}");
        let n = line_ending_note(b"\xEF\xBB\xBFecho hi\n", "my script.sh").unwrap();
        assert!(n.contains("byte order mark") && n.contains("'my script.sh'"), "{n}");
    }
}
