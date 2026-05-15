pub type SubmissionId = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub id: SubmissionId,
    pub op: Op,
}

impl Submission {
    pub fn new(id: impl Into<SubmissionId>, op: Op) -> Self {
        Self { id: id.into(), op }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    UserInput { content: String },
    LocalCommand { command: LocalCommand },
}

impl Op {
    pub fn user_input(content: impl Into<String>) -> Self {
        Self::UserInput {
            content: content.into(),
        }
    }

    pub fn local_command(command: LocalCommand) -> Self {
        Self::LocalCommand { command }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalCommand {
    New,
    Resume,
    Continue,
    Config,
    Image { path: String },
    ImageClipboard,
    Compact,
    Export,
    Share,
    Capabilities,
    Capability { query: String },
    CapabilityEnable { query: String },
    CapabilityDisable { query: String },
    Skill { query: String },
    Tree,
    Fork,
    Session { id: String },
    Model,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submission_wraps_id_and_operation() {
        let submission = Submission::new("sub-0", Op::user_input("hello"));

        assert_eq!(submission.id, "sub-0");
        assert_eq!(
            submission.op,
            Op::UserInput {
                content: "hello".to_owned()
            }
        );
    }
}
