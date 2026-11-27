use super::{color, print_line};
use clv::print;

// NUL separates columns and newline separates entries in the packed help text.
macro_rules! entries {
    ($($entry:ident { $($syntax:literal => $description:literal),+ $(,)? }),+ $(,)?) => {
        #[derive(Clone, Copy)]
        pub(super) enum Entry { $($entry),+ }
        const TEXT: &[u8] = concat!(
            $(
                $($syntax, "\0", $description, "\0",)+
                "\n",
            )+
        ).as_bytes();
    };
}

entries! {
    Shell {
        "help" => "List commands and options",
        "exit" => "Exit the shell",
    },

    FileList {
        "pwd" => "Show the current directory",
        "ls [path]" => "List directory entries",
    },
    Cd { "cd <path>" => "Change directory" },
    Mkdir { "mkdir <path>" => "Create a directory" },
    Rmdir { "rmdir <path>" => "Remove an empty directory" },
    Cat { "cat <path>" => "Print file contents" },
    Write { "write <path> <text>" => "Create or overwrite a file with one line" },
    Rm { "rm <path>" => "Remove a file" },
    Grep { "grep -F <pattern> <path>" => "Print matching lines (literal search)" },

    Run {
        "run <program>" => "Start a child program",
        "run hello <dir>" => "Let hello read hello.txt from <dir>",
    },
    Ask {
        "ask [--echo | --read] <program> <text>" =>
            "Send a request and print the reply (default --echo)",
    },
    AskAt {
        "askat [--read | --echo] hello <dir> <payload>" =>
            "Send a request with a directory grant (default --read)",
    },
    AskFile {
        "askfile [--stream] hello <path>" => "Print file data from one reply or a stream",
    },
    Recv { "recv <program>" => "Collect a child reply" },
    Wait { "wait <program>" => "Wait for the child's result" },
    Event { "event" => "Get the next completion event" },

    GoalCreate {
        "goal create [options]" => "Open a goal",
        "  --children 1|2|3" => "Limit child launches",
        "  --timeout-ms N" => "Expire the goal after N milliseconds",
        "  --reply-bytes N" => "Limit reply bytes",
        "  --read-bytes N" => "Limit all file reads in bytes",
        "  --read-under <dir>" => "Limit file reads to this directory tree",
        "  --helper-read-bytes N" => "Limit helper reads in bytes",
        "  --preserve-file <path>" => "Save input for rename review",
        "  --rename-under <dir>" => "Limit renames to this directory tree",
        "  --allow-helper <program> | --allow-tool <name@rev>" =>
            "Allow one helper or one tool, e.g. text.search@1",
    },
    GrantFile {
        "goal grant-file [mode] <path>" => "Grant a read-only file. Choose at most one mode",
        "  --stage-rename" => "Allow the agent to stage renames",
        "  --delegable" => "Allow any helper to receive it",
        "  --delegate-to <program> [--argument <literal>]" =>
            "Choose the helper and optionally its argument",
    },
    RevokeFile { "goal revoke-file" => "Revoke the grant and discard its staged rename" },
    GoalRun { "goal run [--reply-bytes N] <program>" => "Start a child in the goal" },
    GoalObserve { "goal observe echo" => "Read goal state, deadline and reply budget through echo" },
    StageRename { "goal stage-rename <source> <new-name>" => "Stage a file rename" },
    GoalActions {
        "goal diff" => "Preview or review a staged rename",
        "goal commit" => "Publish the rename before the review expires",
        "goal discard" => "Drop the staged rename",
        "goal status" => "Show goal state",
        "goal budget" => "Show the reply-byte balance",
    },
    Audit {
        "goal audit [--replies | --reads | --renames | --publication]" =>
            "Show audit records (default is delegations)",
    },
    GoalEnd {
        "goal cancel" => "Stop live children and discard a staged rename",
        "goal close" => "Release a ready or finished goal",
    },

    AgentRun {
        "agent run [--suspended] [--reply-bytes N] <program> [request]" =>
            "Start an agent. Requests below apply to hello",
        "  --stage-rename <new-name>" => "Stage a rename of the granted file",
        "  --mock find-greeting[@REV]" => "Run revision 1 (default) or 2",
        "  --delegate --with-file --reply-bytes N grep -F <pattern>" =>
            "Ask grep to search the file",
    },
    Delegate {
        "agent delegate [--with-file] --reply-bytes N <program>" =>
            "Launch a hello or echo helper",
        "agent delegate --with-file --reply-bytes N grep -F <pattern>" =>
            "Launch grep to search the granted file",
    },
    AgentActions {
        "agent status" => "Show agent state and handles",
        "agent suspend" => "Pause the agent",
        "agent resume" => "Resume the agent",
    },
}

pub(super) fn all() {
    use Entry::*;
    section(b"Shell", &[Shell]);
    print(b"  Replace <values>. Brackets mark optional items. | separates choices.\r\n");
    section(
        b"Files and directories",
        &[FileList, Cd, Mkdir, Rmdir, Cat, Write, Rm, Grep],
    );
    section(
        b"Programs and messages",
        &[Run, Ask, AskAt, AskFile, Recv, Wait, Event],
    );
    print(b"\r\n  Built-in programs");
    for &(_, name) in clv_abi::PROGRAMS {
        print(b"  ");
        print(name.as_bytes());
    }
    print(b"\r\n");
    goals();
    agents();
}

pub(super) fn goals() {
    use Entry::*;
    section(
        b"Goals",
        &[
            GoalCreate,
            GrantFile,
            RevokeFile,
            GoalRun,
            GoalObserve,
            StageRename,
            GoalActions,
            Audit,
            GoalEnd,
        ],
    );
}

pub(super) fn agents() {
    use Entry::*;
    section(b"Agents", &[AgentRun, Delegate, AgentActions]);
}

fn section(title: &[u8], entries: &[Entry]) {
    print(b"\r\n");
    color::print_line(color::GREEN, title);
    for entry in entries {
        entry.print(b"  ");
    }
}

impl Entry {
    pub(super) fn usage(self) {
        self.print(b"usage: ");
    }

    fn print(self, mut prefix: &[u8]) {
        const PADDING: [u8; 30] = [b' '; 30];
        let Some(rows) = TEXT.split(|&byte| byte == b'\n').nth(self as usize) else {
            return;
        };
        let mut fields = rows.split(|&byte| byte == 0);
        while let (Some(syntax), Some(description)) = (fields.next(), fields.next()) {
            let gap: &[u8] = if syntax.len() <= PADDING.len() - 2 {
                &PADDING[syntax.len()..]
            } else {
                b"\r\n      "
            };
            print_line(&[prefix, syntax, gap, description]);
            prefix = b"  ";
        }
    }
}
