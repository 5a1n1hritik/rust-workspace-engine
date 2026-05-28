// miller_core/src/orchestrator.rs

use crate::compiler::Language;

/// Build a repair prompt from a previously failed code attempt.
/// The `error_type` string is surfaced prominently so the LLM can triage the failure
/// category before attempting a fix.
pub fn build_repair_prompt(
    original_task: &str,
    broken_code: &str,
    error_log: &str,
    error_type: &str,
) -> String {
    format!(
        r#"You are an expert systems engineer. The previous code you generated failed with a {error_type}.

ORIGINAL TASK:
{original_task}

FAILED CODE:
[CODE_START]
{broken_code}
[CODE_END]

{error_type} LOG:
{error_log}

Fix the issue completely. Return the ENTIRE corrected code strictly inside [CODE_START] and [CODE_END] tags without any markdown code fences."#,
        error_type = error_type,
        original_task = original_task,
        broken_code = broken_code,
        error_log = error_log,
    )
}

/// Build the initial generation prompt, injecting optional retrieved context and
/// indicating the target language so the LLM produces the correct runtime artefact.
pub fn construct_initial_prompt(
    context_code: &str,
    original_task: &str,
    language: &Language,
) -> String {
    let lang_label = match language {
        Language::Rust   => "Rust",
        Language::Python => "Python",
    };

    let fence = match language {
        Language::Rust   => "rust",
        Language::Python => "python",
    };

    format!(
        "You are Miller, an expert {lang} engineer. Return ONLY the executable {lang} code requested.\n\
        You MUST wrap the code inside a standard markdown code block like this:\n\
        ```{fence}\n\
        // code here\n\
        ```\n\
        Do not include any introductory or concluding text.\n\
        {context}\n\
        Task: {task}",
        lang    = lang_label,
        fence   = fence,
        context = context_code,
        task    = original_task,
    )
}
































// // miller_core/src/orchestrator.rs
// pub fn build_repair_prompt(original_task: &str, broken_code: &str, error_log: &str, error_type: &str) -> String {
//     format!(
// r#"You are an expert systems engineer. The previous Rust code you generated failed during {}.

// ORIGINAL TASK:
// {}

// FAILED CODE:
// [CODE_START]
// {}
// [CODE_END]

// {} LOG:
// {}

// Fix the issue completely. Return the ENTIRE updated Rust code strictly inside [CODE_START] and [CODE_END] tags without any markdown code fences."#,
//         error_type, original_task, broken_code, error_type, error_log
//     )
// }

// pub fn construct_initial_prompt(context_code: &str, original_task: &str) -> String {
//     format!(
//         "You are Miller, an expert Rust engineer. Return ONLY the executable Rust code requested.\n\
//         You MUST wrap the code inside a standard markdown code block like this:\n\
//         ```rust\n\
//         // code here\n\
//         ```\n\
//         Do not include any introductory or concluding text.\n\
//         {}\n\
//         Task: {}",
//         context_code,
//         original_task
//     )
// }