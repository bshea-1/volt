use clap::{Parser, Subcommand};
use std::fs;
use std::path::PathBuf;
use volt_core::analysis::ReactivityAnalyzer;
use volt_core::codegen::{DtsEmitter, WasmGcCompiler};
use volt_core::syntax;

#[derive(Parser)]
#[command(name = "volt")]
#[command(about = "Volt: High-Performance, Reactive Web Language Targeting Wasm-GC", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Compile a .vt source file into a Wasm-GC module (.wasm)
    Build {
        /// Input .vt source file
        input: PathBuf,

        /// Output .wasm file path
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Optional output path for TypeScript declarations (.d.ts)
        #[arg(long)]
        dts: Option<PathBuf>,
    },

    /// Parse and validate a .vt source file
    Check {
        /// Input .vt source file
        input: PathBuf,
    },

    /// Serve an interactive development directory with Wasm-GC support
    Serve {
        /// Port to listen on
        #[arg(short, long, default_value_t = 3000)]
        port: u16,

        /// Directory to serve
        #[arg(short, long, default_value = "examples")]
        dir: PathBuf,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Build { input, output, dts } => {
            let source = fs::read_to_string(&input)?;
            let program = syntax::parse(&source)
                .map_err(|e| format!("Syntax Error in {}: {}", input.display(), e))?;

            let mut extern_blocks = Vec::new();
            let mut component_plan = None;

            for item in program.items {
                match item {
                    syntax::TopLevelItem::ExternBlock(eb) => extern_blocks.push(eb),
                    syntax::TopLevelItem::Component(c) => {
                        let plan = ReactivityAnalyzer::analyze_component(&c)
                            .map_err(|e| format!("Analysis Error in {}: {}", c.name, e))?;
                        component_plan = Some(plan);
                    }
                }
            }

            let plan = component_plan
                .ok_or_else(|| format!("No component found in {}", input.display()))?;

            let comp_name = plan.name.clone();
            let mut compiler = WasmGcCompiler::new(plan.clone(), extern_blocks);
            let wasm_bytes = compiler
                .compile()
                .map_err(|e| format!("Codegen Error in {}: {}", comp_name, e))?;

            let wasm_out_path = output.unwrap_or_else(|| input.with_extension("wasm"));
            fs::write(&wasm_out_path, &wasm_bytes)?;
            println!(
                "Successfully compiled {} -> {} ({} bytes)",
                input.display(),
                wasm_out_path.display(),
                wasm_bytes.len()
            );

            if let Some(dts_path) = dts {
                let dts_content = DtsEmitter::emit(&plan);
                fs::write(&dts_path, dts_content)?;
                println!("Generated TypeScript declarations -> {}", dts_path.display());
            }
        }
        Commands::Check { input } => {
            let source = fs::read_to_string(&input)?;
            let program = syntax::parse(&source)
                .map_err(|e| format!("Syntax Error in {}: {}", input.display(), e))?;

            let mut count = 0;
            for item in program.items {
                if let syntax::TopLevelItem::Component(c) = item {
                    let plan = ReactivityAnalyzer::analyze_component(&c)
                        .map_err(|e| format!("Analysis Error in {}: {}", c.name, e))?;
                    println!(
                        "Component '{}' verified: {} signals, {} computeds, {} elements, {} dynamic slots",
                        plan.name,
                        plan.signals.len(),
                        plan.computeds.len(),
                        plan.elements.len(),
                        plan.dynamic_text_slots
                    );
                    count += 1;
                }
            }
            println!("Check passed: {} component(s) valid.", count);
        }
        Commands::Serve { port, dir } => {
            println!("Serving directory '{}' on http://localhost:{}", dir.display(), port);
            println!("Open http://localhost:{}/index.html in Chrome (119+), Firefox (120+), or Safari (18+)", port);
            serve_directory(dir, port)?;
        }
    }

    Ok(())
}

fn serve_directory(dir: PathBuf, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind(format!("0.0.0.0:{}", port))?;
    println!("Server running on http://127.0.0.1:{}", port);

    for stream in listener.incoming() {
        let mut stream = stream?;
        let mut buffer = [0; 2048];
        let bytes_read = stream.read(&mut buffer)?;
        let request = String::from_utf8_lossy(&buffer[..bytes_read]);

        let path = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/");

        let relative_path = if path == "/" {
            "index.html"
        } else {
            path.trim_start_matches('/')
        };

        let file_path = dir.join(relative_path);
        if file_path.exists() && file_path.is_file() {
            let contents = fs::read(&file_path)?;
            let mime = if file_path.extension().and_then(|s| s.to_str()) == Some("wasm") {
                "application/wasm"
            } else if file_path.extension().and_then(|s| s.to_str()) == Some("html") {
                "text/html; charset=utf-8"
            } else if file_path.extension().and_then(|s| s.to_str()) == Some("js") {
                "application/javascript; charset=utf-8"
            } else if file_path.extension().and_then(|s| s.to_str()) == Some("css") {
                "text/css; charset=utf-8"
            } else {
                "application/octet-stream"
            };

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: *\r\n\r\n",
                mime,
                contents.len()
            );
            stream.write_all(response.as_bytes())?;
            stream.write_all(&contents)?;
        } else {
            let not_found = "HTTP/1.1 404 NOT FOUND\r\nContent-Length: 9\r\n\r\nNot Found";
            stream.write_all(not_found.as_bytes())?;
        }
    }
    Ok(())
}
