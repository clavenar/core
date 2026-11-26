macro_rules! subcommands {
    ($($variant:ident => $name:literal),+ $(,)?) => {
        #[derive(Clone, Copy)]
        pub(super) enum Subcommand { $($variant,)+ Unknown }

        impl Subcommand {
            pub(super) fn parse(word: &[u8]) -> Self {
                // Separate arrays avoid per-entry padding in the boot shell image.
                const NAMES: &[&[u8]] = &[$($name),+];
                const COMMANDS: &[Subcommand] = &[$(Subcommand::$variant),+];
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

subcommands! {
    Create => b"create",
    GrantFile => b"grant-file",
    StageRename => b"stage-rename",
    Diff => b"diff",
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
}
