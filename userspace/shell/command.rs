macro_rules! commands {
    ($kind:ident { $($variant:ident => $name:literal),+ $(,)? }) => {
        #[derive(Clone, Copy)]
        pub(super) enum $kind { $($variant,)+ Unknown }

        impl $kind {
            pub(super) fn parse(word: &[u8]) -> Self {
                // Separate arrays avoid per-entry padding in the boot shell image.
                const NAMES: &[&[u8]] = &[$($name),+];
                const COMMANDS: &[$kind] = &[$($kind::$variant),+];
                for (&name, &command) in NAMES.iter().zip(COMMANDS) {
                    if name == word {
                        return command;
                    }
                }
                Self::Unknown
            }
        }
    };
}

commands!(Command {
    Help => b"help",
    Pwd => b"pwd",
    Cd => b"cd",
    Ls => b"ls",
    Mkdir => b"mkdir",
    Rmdir => b"rmdir",
    Cat => b"cat",
    Write => b"write",
    Rm => b"rm",
    Run => b"run",
    Ask => b"ask",
    AskAt => b"askat",
    AskFile => b"askfile",
    Grep => b"grep",
    Recv => b"recv",
    Wait => b"wait",
    Event => b"event",
    Goal => b"goal",
    Agent => b"agent",
    Exit => b"exit",
    Empty => b"",
});

commands!(Subcommand {
    Create => b"create",
    GrantFile => b"grant-file",
    StageRename => b"stage-rename",
    Diff => b"diff",
    Commit => b"commit",
    RevokeFile => b"revoke-file",
    Run => b"run",
    Audit => b"audit",
    Budget => b"budget",
    Status => b"status",
    Cancel => b"cancel",
    Close => b"close",
    Delegate => b"delegate",
    Suspend => b"suspend",
    Resume => b"resume",
});
