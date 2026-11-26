macro_rules! keywords {
    ($kind:ident { $($variant:ident => $name:literal),+ $(,)? }) => {
        #[derive(Clone, Copy)]
        pub(super) enum $kind { $($variant,)+ Unknown }

        impl $kind {
            pub(super) fn parse(word: &[u8]) -> Self {
                // Store one NUL per spelling to keep the boot shell's lookup tables compact.
                const NAMES: &[u8] = concat!($($name, "\0",)+).as_bytes();
                const VALUES: &[$kind] = &[$($kind::$variant),+];
                for (name, &value) in NAMES.split(|&byte| byte == 0).zip(VALUES) {
                    if name == word {
                        return value;
                    }
                }
                Self::Unknown
            }
        }
    };
}

keywords!(Command {
    Help => "help",
    Pwd => "pwd",
    Cd => "cd",
    Ls => "ls",
    Mkdir => "mkdir",
    Rmdir => "rmdir",
    Cat => "cat",
    Write => "write",
    Rm => "rm",
    Run => "run",
    Ask => "ask",
    AskAt => "askat",
    AskFile => "askfile",
    Grep => "grep",
    Recv => "recv",
    Wait => "wait",
    Event => "event",
    Goal => "goal",
    Agent => "agent",
    Exit => "exit",
    Empty => "",
});

keywords!(Subcommand {
    Create => "create",
    GrantFile => "grant-file",
    StageRename => "stage-rename",
    Diff => "diff",
    Commit => "commit",
    Discard => "discard",
    RevokeFile => "revoke-file",
    Run => "run",
    Audit => "audit",
    Budget => "budget",
    Status => "status",
    Cancel => "cancel",
    Close => "close",
    Delegate => "delegate",
    Suspend => "suspend",
    Resume => "resume",
});

keywords!(GoalFlag {
    ReplyBytes => "--reply-bytes",
    Children => "--children",
    ReadBytes => "--read-bytes",
    HelperReadBytes => "--helper-read-bytes",
    AllowHelper => "--allow-helper",
    AllowTool => "--allow-tool",
    PreserveFile => "--preserve-file",
    RenameUnder => "--rename-under",
    Suspended => "--suspended",
    WithFile => "--with-file",
    Delegable => "--delegable",
    DelegateTo => "--delegate-to",
    Argument => "--argument",
    Delegate => "--delegate",
    Mock => "--mock",
    StageRename => "--stage-rename",
});
