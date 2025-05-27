use anyhow::{Context, Result, anyhow};
use clap::Parser;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

#[derive(Debug, Parser)]
#[command(name = "SlimeOS Kernel Runner")]
#[command(version = "0.1.0")]
#[command(about = "Builds and runs SlimeOS kernel with QEMU")]
struct Args {
    /// Path to kernel binary
    kernel: String,

    /// Output directory for disk images
    #[arg(
        short = 'o',
        long = "output",
        value_name = "DIR",
        default_value = "target"
    )]
    output: String,

    /// Enable QEMU debugging (adds -s -S flags)
    #[arg(short = 'd', long = "debug")]
    debug: bool,

    /// Additional QEMU arguments (space-separated string)
    #[arg(short = 'a', long = "qemu-args", value_name = "ARG")]
    qemu_args: Option<String>,

    /// Build disk image but don't start QEMU
    #[arg(long = "no-run")]
    no_run: bool,
}

#[derive(Debug)]
struct Config {
    kernel_path: PathBuf,
    output_dir: PathBuf,
    uefi_path: PathBuf,
    enable_debug: bool,
    no_run: bool,
    qemu_args: Vec<String>,
}

impl Config {
    fn from_args(args: Args) -> Result<Self> {
        let kernel_path = Self::resolve_kernel_path(&args.kernel)?;
        let output_dir = Self::resolve_output_dir(&args.output)?;
        let uefi_path = output_dir.join("uefi.img");

        let qemu_args = args
            .qemu_args
            .map(|args| args.split_whitespace().map(String::from).collect())
            .unwrap_or_default();

        println!("{:?}",qemu_args);

        Ok(Self {
            kernel_path,
            output_dir,
            uefi_path,
            enable_debug: args.debug,
            no_run: args.no_run,
            qemu_args,
        })
    }

    fn resolve_kernel_path(path_str: &str) -> Result<PathBuf> {
        let path = PathBuf::from(path_str)
            .canonicalize()
            .with_context(|| format!("Failed to resolve kernel path: {}", path_str))?;

        if !path.exists() {
            return Err(anyhow!("Kernel binary not found at: {}", path.display()));
        }

        Ok(path)
    }

    fn resolve_output_dir(dir_str: &str) -> Result<PathBuf> {
        let dir = PathBuf::from(dir_str);

        if !dir.exists() {
            fs::create_dir_all(&dir)
                .with_context(|| format!("Failed to create output directory: {}", dir_str))?;
        }

        dir.canonicalize()
            .with_context(|| format!("Failed to resolve output directory: {}", dir_str))
    }
}

struct KernelRunner {
    config: Config,
}

impl KernelRunner {
    fn new(config: Config) -> Self {
        Self { config }
    }

    fn run(&self) -> Result<()> {
        self.print_configuration();
        self.build_artifacts()?;

        if self.config.no_run {
            println!(
                "Disk image built successfully at: {}",
                self.config.uefi_path.display()
            );
        } else {
            println!("Starting QEMU...");
            self.start_qemu()?;
        }

        Ok(())
    }

    fn print_configuration(&self) {
        println!("Building disk images...");
        println!("Kernel: {}", self.config.kernel_path.display());
        println!("Output: {}", self.config.output_dir.display());
    }

    fn build_artifacts(&self) -> Result<()> {
        self.build_disk_image()?;
        self.build_debug_script()?;
        Ok(())
    }

    fn build_disk_image(&self) -> Result<()> {
        bootloader::UefiBoot::new(&self.config.kernel_path)
            .create_disk_image(&self.config.uefi_path)
            .with_context(|| {
                format!(
                    "Failed to create UEFI disk image at: {}",
                    self.config.uefi_path.display()
                )
            })?;

        println!(
            "Created UEFI disk image: {}",
            self.config.uefi_path.display()
        );
        Ok(())
    }

    fn build_debug_script(&self) -> Result<()> {
        let debug_script = DebugScript::new(&self.config.kernel_path);
        debug_script.generate()?;
        Ok(())
    }

    fn start_qemu(&self) -> Result<()> {
        let qemu = QemuRunner::new(&self.config);
        qemu.execute()
    }
}

struct DebugScript {
    kernel_path: PathBuf,
    script_path: PathBuf,
}

impl DebugScript {
    fn new(kernel_path: &Path) -> Self {
        Self {
            kernel_path: kernel_path.to_path_buf(),
            script_path: PathBuf::from("../debug.sh"),
        }
    }

    fn generate(&self) -> Result<()> {
        let content = self.generate_content();

        fs::write(&self.script_path, content).with_context(|| {
            format!(
                "Failed to write debug script to {}",
                self.script_path.display()
            )
        })?;

        self.make_executable()?;
        println!("Generated debug script: {}", self.script_path.display());
        Ok(())
    }

    fn generate_content(&self) -> String {
        format!(
            r#"#!/bin/bash
# Auto-generated debug script for SlimeOS kernel
# Generated by SlimeOS Kernel Runner - do not edit manually

set -euo pipefail

readonly KERNEL_PATH="{}"
readonly LLDB_CMD="${{LLDB_CMD:-rust-lldb}}"
readonly GDB_PORT="${{GDB_PORT:-1234}}"

if [[ ! -f "$KERNEL_PATH" ]]; then
    echo "Error: Kernel binary not found at $KERNEL_PATH" >&2
    exit 1
fi

echo "Starting LLDB debugging session for SlimeOS kernel..."
echo "Kernel: $KERNEL_PATH"
echo "GDB remote port: $GDB_PORT"
echo ""
echo "Make sure QEMU is running with debugging enabled (-s -S flags)"
echo "Run the kernel runner with: --debug"
echo ""

exec "$LLDB_CMD" \
    -o "target create \"$KERNEL_PATH\"" \
    -o "target modules load --file \"$KERNEL_PATH\" --slide 0x8000000000" \
    -o "gdb-remote localhost:$GDB_PORT" \
    -o "b _start" \
    -o "c"
"#,
            self.kernel_path.display()
        )
    }

    #[cfg(unix)]
    fn make_executable(&self) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = fs::metadata(&self.script_path)
            .with_context(|| {
                format!(
                    "Failed to get permissions for {}",
                    self.script_path.display()
                )
            })?
            .permissions();

        perms.set_mode(0o755);
        fs::set_permissions(&self.script_path, perms).with_context(|| {
            format!(
                "Failed to set permissions for {}",
                self.script_path.display()
            )
        })?;

        Ok(())
    }

    #[cfg(not(unix))]
    fn make_executable(&self) -> Result<()> {
        // No-op on non-Unix systems
        Ok(())
    }
}

struct QemuRunner<'a> {
    config: &'a Config,
}

impl<'a> QemuRunner<'a> {
    fn new(config: &'a Config) -> Self {
        Self { config }
    }

    fn execute(&self) -> Result<()> {
        let mut cmd = self.build_command();
        self.print_command_info(&cmd);
        self.run_command(&mut cmd)
    }

    fn build_command(&self) -> process::Command {
        let mut cmd = process::Command::new("qemu-system-x86_64");

        // Basic QEMU configuration
        cmd.args(["-bios", ovmf_prebuilt::ovmf_pure_efi().to_str().unwrap()]);
        cmd.args([
            "-drive",
            &format!("format=raw,file={}", self.config.uefi_path.display()),
        ]);

        // Standard arguments
        cmd.args([
            "-device",
            "isa-debug-exit,iobase=0xf4,iosize=0x04",
            "-serial",
            "stdio",
        ]);

        // Debug support
        if self.config.enable_debug {
            cmd.args(["-s", "-S"]);
        }

        // Additional user arguments
        if !self.config.qemu_args.is_empty() {
            cmd.args(&self.config.qemu_args);
        }

        cmd
    }

    fn print_command_info(&self, cmd: &process::Command) {
        if self.config.enable_debug {
            println!("Debug mode enabled: GDB server on port 1234, CPU halted at startup");
            println!("Run ../debug.sh in another terminal to connect debugger");
        }

        if !self.config.qemu_args.is_empty() {
            println!("Additional QEMU args: {}", self.config.qemu_args.join(" "));
        }

        println!("BIOS: {}", ovmf_prebuilt::ovmf_pure_efi().display());
        println!("Drive: {}", self.config.uefi_path.display());
        println!("Command: {:?}", cmd);
    }

    fn run_command(&self, cmd: &mut process::Command) -> Result<()> {
        let mut child = cmd
            .spawn()
            .with_context(|| "Failed to start QEMU - ensure qemu-system-x86_64 is installed")?;

        let exit_status = child
            .wait()
            .with_context(|| "QEMU process terminated unexpectedly")?;

        if exit_status.success() {
            Ok(())
        } else {
            Err(anyhow!(
                "QEMU exited with error code: {:?}",
                exit_status.code()
            ))
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = Config::from_args(args)?;
    let runner = KernelRunner::new(config);
    runner.run()
}