❯ cargo run
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s
     Running `/home/hritik/Documents/workspace-p/miller_workspace/target/debug/miller_core`
[Memory] Collection 'miller_codebase' already active.
=== MILLER: Local Autonomous Coding Framework ===
[System] No external workspace attached. Background scanner is Idle.

Miller ko task batao:
> Given two sorted arrays nums1 and nums2 of size m and n respectively, return the median of the two sorted arrays.

The overall run time complexity should be o(log(m+n)).

example 1:

Input: nums1 = [1,3], nums2 = [2]

output: 2.00000

explanation: merg[Retrieval] Searching past code patterns from local Qdrant DB...
ed array = [1,2,3] and median is 2. 



example 2:

Input: nums1 = [1,2], nums2 = [3,4]

Output: 2.50000

Explanation: merged array = [1,2,3,4] and median is (2 + 3 ) / 2 = 2.5.[Retrieval Match] Valid structural matches found. Injecting blocks...

[Miller] Generating code (Attempt 1/5)...
[Filesystem] Code successfully written to sandbox: 'sandbox.rs'
[Compiler] Running rustc validation...
[Success] Code compiled successfully! Moving to Execution Sandbox...

[Sandbox Execution Pass]
--- STDOUT ---
Median: 2

--------------

Processing Loop Cycle Ended Successfully.
❯ 
❯ The overall run time complexity should be o(log(m+n)).
zsh: no matches found: o(log(m+n)).
❯ 
❯ example 1:
zsh: command not found: example
❯ 
❯ Input: nums1 = [1,3], nums2 = [2]
zsh: no matches found: [1,3],
❯ 
❯ output: 2.00000
zsh: command not found: output:
❯ 
❯ explanation: merged array = [1,2,3] and median is 2.
zsh: no matches found: [1,2,3]
❯ 
❯ 
❯ 
❯ example 2:
zsh: command not found: example
❯ 
❯ Input: nums1 = [1,2], nums2 = [3,4]
zsh: no matches found: [1,2],
❯ 
❯ Output: 2.50000
zsh: command not found: Output:
❯ 


---

# Test 2 
```text
❯ cargo run
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.27s
     Running `/home/hritik/Documents/workspace-p/miller_workspace/target/debug/miller_core`
[Memory] Collection 'miller_codebase' already active.
=== MILLER: Local Autonomous Coding Framework ===
[System] No external workspace attached. Background scanner is Idle.

Miller ko task batao:
> write a function to add the two sorted array. 
[Retrieval] Searching past code patterns from local Qdrant DB...
[Retrieval Match] Valid structural matches found. Injecting blocks...

[Miller] Generating code (Attempt 1/5)...
[Filesystem] Code successfully written to sandbox: 'sandbox.rs'
[Compiler] Running rustc validation...
[Success] Code compiled successfully! Moving to Execution Sandbox...

[Sandbox Execution Pass]
--- STDOUT ---
Merged Array: [1, 2, 3, 4, 5, 6]

--------------

Processing Loop Cycle Ended Successfully.

❯ cat sandbox.rs
// Function to add two sorted arrays of integers and return a new sorted array
fn merge_sorted_arrays(arr1: &[i32], arr2: &[i32]) -> Vec<i32> {
    let mut result = Vec::new();
    let mut i = 0;
    let mut j = 0;

    while i < arr1.len() && j < arr2.len() {
        if arr1[i] <= arr2[j] {
            result.push(arr1[i]);
            i += 1;
        } else {
            result.push(arr2[j]);
            j += 1;
        }
    }

    // Append remaining elements from arr1, if any
    while i < arr1.len() {
        result.push(arr1[i]);
        i += 1;
    }

    // Append remaining elements from arr2, if any
    while j < arr2.len() {
        result.push(arr2[j]);
        j += 1;
    }

    result
}

fn main() {
    let array1 = [1, 3, 5];
    let array2 = [2, 4, 6];

    let merged_array = merge_sorted_arrays(&array1, &array2);
    println!("Merged Array: {:?}", merged_array); // Output: [1, 2, 3, 4, 5, 6]
}%
```
---

# Test 3
```text
❯ cargo run -- /home/hritik/Documents/workspace-p/miller_workspace
   Compiling miller_parser v0.1.0 (/home/hritik/Documents/workspace-p/miller_workspace/miller_parser)
   Compiling miller_memory v0.1.0 (/home/hritik/Documents/workspace-p/miller_workspace/miller_memory)
   Compiling miller_core v0.1.0 (/home/hritik/Documents/workspace-p/miller_workspace/miller_core)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.34s
     Running `/home/hritik/Documents/workspace-p/miller_workspace/target/debug/miller_core /home/hritik/Documents/workspace-p/miller_workspace`
[13:17:37] [INFO] [MEMORY] Collection 'miller_codebase' already active.
[13:17:37] [INFO] [SYSTEM] === MILLER: Local Autonomous Coding Framework ===
[13:17:37] [INFO] [SYSTEM] External project path loaded: "/home/hritik/Documents/workspace-p/miller_workspace"

Miller ko task batao:
> [13:17:37] [INFO] [BACKGROUND] Silent monitoring loop active: "/home/hritik/Documents/workspace-p/miller_workspace"
[13:17:37] [INFO] [BACKGROUND] Incremental background indexing scaning...
[13:17:37] [INFO] [PARSER] New file or change detected. Extracting code: "/home/hritik/Documents/workspace-p/miller_workspace/miller_parser/src/lib.rs"
[13:17:37] [INFO] [PARSER] New file or change detected. Extracting code: "/home/hritik/Documents/workspace-p/miller_workspace/miller_parser/src/ast_graph.rs"
[13:17:37] [INFO] [PARSER] New file or change detected. Extracting code: "/home/hritik/Documents/workspace-p/miller_workspace/miller_parser/src/scanner.rs"
[13:17:37] [INFO] [PARSER] New file or change detected. Extracting code: "/home/hritik/Documents/workspace-p/miller_workspace/miller_memory/src/lib.rs"
[13:17:37] [INFO] [PARSER] New file or change detected. Extracting code: "/home/hritik/Documents/workspace-p/miller_workspace/miller_core/src/main.rs"
[13:17:37] [INFO] [PARSER] New file or change detected. Extracting code: "/home/hritik/Documents/workspace-p/miller_workspace/miller_core/src/security.rs"
[13:17:37] [INFO] [PARSER] New file or change detected. Extracting code: "/home/hritik/Documents/workspace-p/miller_workspace/miller_core/src/config.rs"
[13:17:37] [INFO] [PARSER] New file or change detected. Extracting code: "/home/hritik/Documents/workspace-p/miller_workspace/miller_core/src/compiler.rs"
[13:17:37] [INFO] [PARSER] New file or change detected. Extracting code: "/home/hritik/Documents/workspace-p/miller_workspace/miller_core/src/orchestrator.rs"
[13:17:37] [INFO] [PARSER] New file or change detected. Extracting code: "/home/hritik/Documents/workspace-p/miller_workspace/miller_core/src/network.rs"
[13:17:37] [INFO] [PARSER] New file or change detected. Extracting code: "/home/hritik/Documents/workspace-p/miller_workspace/miller_core/src/logger.rs"
[13:17:37] [INFO] [PARSER] Scan Done. Total Checked: 11 | Skipped (Unchanged): 0 | Extracted New Items: 35
[13:17:37] [INFO] [BACKGROUND] Syncing 35 modified items into local Qdrant...
[13:17:39] [INFO] [BACKGROUND] Vector embedding database sync cycle completed!


```

# Test 4
```text
❯ cargo run -- /home/hritik/Documents/workspace-p/wildlife-data-collector
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.09s
     Running `/home/hritik/Documents/workspace-p/miller_workspace/target/debug/miller_core /home/hritik/Documents/workspace-p/wildlife-data-collector`
[13:18:34] [INFO] [MEMORY] Collection 'miller_codebase' already active.
[13:18:34] [INFO] [SYSTEM] === MILLER: Local Autonomous Coding Framework ===
[13:18:34] [INFO] [SYSTEM] External project path loaded: "/home/hritik/Documents/workspace-p/wildlife-data-collector"

Miller ko task batao:
> [13:18:34] [INFO] [BACKGROUND] Silent monitoring loop active: "/home/hritik/Documents/workspace-p/wildlife-data-collector"
[13:18:34] [INFO] [BACKGROUND] Incremental background indexing scaning...
[13:18:34] [INFO] [PARSER] Scan Done. Total Checked: 0 | Skipped (Unchanged): 0 | Extracted New Items: 0
[13:18:34] [INFO] [BACKGROUND] Cache clean. Workspace state sync complete.

```

