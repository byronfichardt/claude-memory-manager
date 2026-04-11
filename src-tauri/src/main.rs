#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Hook mode: called by Claude Code's UserPromptSubmit hook.
    // Reads JSON from stdin, queries the memory store, writes context to stdout.
    if args.iter().any(|a| a == "--hook") {
        match claude_memory_manager_lib::run_hook() {
            Ok(()) => return,
            Err(e) => {
                eprintln!("Hook error: {}", e);
                std::process::exit(0); // exit 0 = allow, don't block the user's prompt
            }
        }
    }

    // MCP server mode: spawned by Claude Code as a stdio MCP server
    if args.iter().any(|a| a == "--mcp-server") {
        if let Err(e) = claude_memory_manager_lib::run_mcp_server() {
            eprintln!("MCP server error: {}", e);
            std::process::exit(1);
        }
        return;
    }

    claude_memory_manager_lib::run()
}
