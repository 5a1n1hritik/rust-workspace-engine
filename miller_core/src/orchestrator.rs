// miller_core/src/orchestrator.rs

use crate::compiler::Language;

// ── Prompt constants ──────────────────────────────────────────────────────────

/// The file-tagging format the LLM is instructed to use. Kept as a single
/// source-of-truth constant so the parser in main.rs can reference the same
/// literal markers without duplication.
pub const FILE_TAG_OPEN:  &str = "<file path=\"";
pub const FILE_TAG_SEP:   &str = "\">";
pub const FILE_TAG_CLOSE: &str = "</file>";

// ── Shared format instruction injected into every prompt ─────────────────────

/// Inline format rules explaining the multi-file tagging protocol to the LLM.
/// Embedded in both initial and repair prompts so the model always has the
/// contract in view regardless of which prompt triggered the generation.
fn file_format_rules(lang_label: &str) -> String {
    format!(
        r#"OUTPUT FORMAT — READ CAREFULLY:
You MUST emit every file using this exact tagging structure:

{open}relative/path/to/file.ext{sep}
<contents of that file here>
{close}

Rules:
1. You may emit as MANY <file ...> blocks as the task requires — one per file.
2. The path attribute MUST be a clean relative path (e.g. "src/main.{ext}", "Cargo.toml").
   Never use leading slashes or "../" traversal sequences.
3. Do NOT wrap file contents in markdown fences or add any extra commentary inside the tags.
4. Do NOT emit any text outside the <file ...> blocks. No preamble, no summary, nothing.
5. For multi-file projects (e.g. a Rust crate), always include ALL required files
   (Cargo.toml, src/main.{ext}, etc.) in one response.
6. If the task only needs a single file, emit exactly one <file ...> block."#,
        open  = FILE_TAG_OPEN,
        sep   = FILE_TAG_SEP,
        close = FILE_TAG_CLOSE,
        ext   = if lang_label == "Rust" { "rs" } else { "py" },
    )
}

// ── Public prompt builders ────────────────────────────────────────────────────

/// Build the initial task prompt. Injects optional retrieved context and the
/// multi-file tagging protocol so the LLM knows exactly how to structure output.
pub fn construct_initial_prompt(
    context_code: &str,
    original_task: &str,
    language: &Language,
) -> String {
    let lang_label = match language {
        Language::Rust   => "Rust",
        Language::Python => "Python",
    };

    let format_rules = file_format_rules(lang_label);

    format!(
        "You are Miller, an expert {lang} engineer.\n\
        {rules}\n\
        {context}\
        Task: {task}",
        lang    = lang_label,
        rules   = format_rules,
        context = if context_code.is_empty() {
            String::new()
        } else {
            format!("{}\n", context_code)
        },
        task    = original_task,
    )
}

/// Build a repair prompt for a previously failed attempt.
///
/// The failed file set is reconstructed as tagged blocks so the model sees the
/// same format it is expected to produce, making the round-trip unambiguous.
/// The `error_type` label is surfaced prominently so the LLM can triage the
/// failure category before attempting a fix.
pub fn build_repair_prompt(
    original_task: &str,
    broken_files_repr: &str,
    error_log: &str,
    error_type: &str,
) -> String {
    let format_rules = file_format_rules("Rust"); // repair is always same language

    format!(
        r#"You are an expert systems engineer. The code you generated previously failed with a {error_type}.

ORIGINAL TASK:
{task}

FAILED FILE SET:
{broken}

{error_type} LOG:
{log}

{rules}

Fix the issue completely. Re-emit ALL files in the corrected <file path="..."> format.
Do NOT omit any file even if it is unchanged."#,
        error_type = error_type,
        task       = original_task,
        broken     = broken_files_repr,
        log        = error_log,
        rules      = format_rules,
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