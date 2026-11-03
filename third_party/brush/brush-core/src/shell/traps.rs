//! Trap handling for the shell.

use crate::{ExecutionParameters, ExecutionResult, ProcessGroupPolicy, error, traps::TrapSignal};

impl<SE: crate::extensions::ShellExtensions> crate::Shell<SE> {
    /// Runs any exit steps for the shell.
    ///
    /// This currently includes invoking the `EXIT` trap handler, if any. If the handler
    /// exits the shell, its exit status becomes the shell's last exit status.
    pub async fn on_exit(&mut self) -> Result<(), error::Error> {
        self.run_exit_trap().await?;
        Ok(())
    }

    /// Invokes the `EXIT` trap handler, if any. If the handler exits the shell, returns
    /// the status it exits with, having made it the shell's last exit status.
    pub(crate) async fn run_exit_trap(&mut self) -> Result<Option<u8>, error::Error> {
        if !self.traps.handles(TrapSignal::Exit) {
            return Ok(None);
        }

        let result = self
            .invoke_trap_handler(TrapSignal::Exit, &self.default_exec_params())
            .await?;

        if result.is_exit() {
            let status = result.exit_code.into();
            self.set_last_exit_status(status);
            Ok(Some(status))
        } else {
            Ok(None)
        }
    }

    /// Invokes the handler registered for `signal`, if any.
    ///
    /// Behavior varies by signal type:
    ///
    /// * **Per-signal recursion guard** — each trap guards against its own self-recursion, but
    ///   different traps *can* fire from within each other's handlers (matching bash semantics).
    ///
    /// * **Inheritance** — in functions and subshells, some traps are only inherited when the
    ///   corresponding shell option is enabled (e.g. `errtrace` / `set -E` for `ERR`, `functrace` /
    ///   `set -T` for `DEBUG`/`RETURN`).
    ///
    /// * **`$?` preservation** — `last_exit_status` is saved before and restored after the handler
    ///   runs so the trap does not clobber the status that triggered it.
    ///
    /// # Arguments
    ///
    /// * `signal`: Signal to run handler for.
    ///
    /// * `params`: Execution parameters to use for handler.
    pub(crate) async fn invoke_trap_handler(
        &mut self,
        signal: TrapSignal,
        params: &ExecutionParameters,
    ) -> Result<ExecutionResult, error::Error> {
        // Per-signal self-recursion guard: don't re-enter a trap that is
        // already being handled. Different traps *can* fire from each
        // other's handlers (e.g. ERR inside EXIT, EXIT inside ERR).
        if self.call_stack().is_trap_signal_active(signal) {
            return Ok(ExecutionResult::success());
        }

        // Don't fire traps that have been explicitly suppressed (e.g. DEBUG
        // during programmable completion).
        if self.call_stack().is_trap_delivery_suppressed() {
            return Ok(ExecutionResult::success());
        }

        let Some(handler) = self.traps.get_handler(signal).cloned() else {
            return Ok(ExecutionResult::success());
        };

        // sill: a subshell runs its own EXIT trap, never its parent's (whose handler it has
        // a copy of).
        if signal == TrapSignal::Exit && handler.owner.shell != self.shell_id() {
            return Ok(ExecutionResult::success());
        }

        // In functions and subshells, some traps are only inherited when the
        // corresponding option is enabled. sill: a trap set in this very function call or
        // subshell isn't inherited, it's its own: `f() { trap '...' RETURN; ...; }` and
        // `( trap '...' ERR; false )` run theirs, as in bash.
        if (self.in_function() || self.is_subshell())
            && !self.is_trap_inherited_in_current_scope(signal)
            && handler.owner.scope != self.current_scope_id()
        {
            return Ok(ExecutionResult::success());
        }

        let mut params = params.clone();
        params.process_group_policy = ProcessGroupPolicy::SameProcessGroup;

        // The guard leaves the handler, restoring `$?`, once this finishes -- or is cancelled.
        self.enter_trap_handler(signal, Some(&handler))
            .run_string(&handler.command, &handler.source_info, &params)
            .await
    }

    /// sill: ends a subshell (`( ... )`, `$(...)`, `<(...)`) as bash does: with the EXIT
    /// trap it set, if any, run with its standard streams (so a `$(...)` captures the trap's
    /// output too). `( tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT; ... )` left the folder
    /// behind. An `exit` in the trap sets the status.
    pub(crate) async fn finish_subshell(
        &mut self,
        result: Result<ExecutionResult, error::Error>,
        params: &ExecutionParameters,
    ) -> Result<ExecutionResult, error::Error> {
        let own_exit_trap = self
            .traps
            .get_handler(TrapSignal::Exit)
            .is_some_and(|h| h.owner.shell == self.shell_id());
        if !self.is_subshell() || !own_exit_trap {
            return result;
        }
        if let Ok(r) = &result {
            self.set_last_exit_status(r.exit_code.into());
        }
        match self.invoke_trap_handler(TrapSignal::Exit, params).await {
            Ok(trap_result) if trap_result.is_exit() => Ok(ExecutionResult::from(trap_result.exit_code)),
            _ => result,
        }
    }

    /// sill: runs the RETURN trap as a function call or a sourced script ends (before its
    /// frame is left), with `$?` its status.
    pub(crate) async fn run_return_trap(&mut self, status: Option<u8>, params: &ExecutionParameters) {
        if !self.traps.handles(TrapSignal::Return) {
            return;
        }
        if let Some(status) = status {
            self.set_last_exit_status(status);
        }
        let _ = self.invoke_trap_handler(TrapSignal::Return, params).await;
    }

    /// Returns whether the given trap signal is inherited in the current
    /// function or subshell scope.
    fn is_trap_inherited_in_current_scope(&self, signal: TrapSignal) -> bool {
        match signal {
            TrapSignal::Err => self.options().shell_functions_inherit_err_trap,
            TrapSignal::Debug | TrapSignal::Return => {
                self.options()
                    .shell_functions_inherit_debug_and_return_traps
            }
            // EXIT and system signals are always inherited — i.e. their visibility is
            // not gated by errtrace/functrace options. (The actual trap *state* for
            // subshells is managed separately via `Shell::clone`.)
            TrapSignal::Exit | TrapSignal::Signal(_) => true,
        }
    }
}
