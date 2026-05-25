// miller_core/src/orchestrator.rs
pub fn build_repair_prompt(original_task: &str, broken_code: &str, error_log: &str, error_type: &str) -> String {
    format!(
r#"You are an expert systems engineer. The previous Rust code you generated failed during {}.

ORIGINAL TASK:
{}

FAILED CODE:
[CODE_START]
{}
[CODE_END]

{} LOG:
{}

Fix the issue completely. Return the ENTIRE updated Rust code strictly inside [CODE_START] and [CODE_END] tags without any markdown code fences."#,
        error_type, original_task, broken_code, error_type, error_log
    )
}

pub fn construct_initial_prompt(context_code: &str, original_task: &str) -> String {
    format!(
        "You are Miller, an expert Rust engineer. Return ONLY the executable Rust code requested.\n\
        You MUST wrap the code inside a standard markdown code block like this:\n\
        ```rust\n\
        // code here\n\
        ```\n\
        Do not include any introductory or concluding text.\n\
        {}\n\
        Task: {}",
        context_code,
        original_task
    )
}