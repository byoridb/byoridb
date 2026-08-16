// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! ByoriDB Backup CLI Tool
//!
//! This tool provides backup and restore functionality for ByoriDB databases.
//!
//! # Usage
//!
//! ```bash
//! # Create a backup
//! byoridb-backup create --db /path/to/data --backup-dir /path/to/backups
//!
//! # Create a backup with label
//! byoridb-backup create --db /path/to/data --backup-dir /path/to/backups --label "daily backup"
//!
//! # List all backups
//! byoridb-backup list --backup-dir /path/to/backups
//!
//! # Restore a backup
//! byoridb-backup restore --backup-dir /path/to/backups --backup-id backup_1234567890 --target /path/to/restore
//!
//! # Delete a backup
//! byoridb-backup delete --backup-dir /path/to/backups --backup-id backup_1234567890
//!
//! # Cleanup old backups (keep only N most recent)
//! byoridb-backup cleanup --backup-dir /path/to/backups --keep 5
//! ```

use anyhow::{Context, Result};
use byoridb_kvstore::{format_bytes, format_timestamp, BackupManager, BackupOptions};
use clap::{Parser, Subcommand};

#[path = "version.rs"]
mod version;
use std::path::PathBuf;
use version::VERSION;

#[derive(Parser)]
#[command(name = "byoridb-backup")]
#[command(author = "ByoriDB")]
#[command(version = VERSION)]
#[command(about = "ByoriDB Backup and Restore Tool", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a new backup
    Create {
        /// Path to the database directory
        #[arg(short, long)]
        db: PathBuf,

        /// Path to the backup directory
        #[arg(short, long)]
        backup_dir: PathBuf,

        /// Optional label for the backup
        #[arg(short, long)]
        label: Option<String>,

        /// Skip WAL flush before backup (faster but may miss recent writes)
        #[arg(long, default_value = "false")]
        no_flush: bool,
    },

    /// List all backups
    List {
        /// Path to the backup directory
        #[arg(short, long)]
        backup_dir: PathBuf,

        /// Output format (table, json)
        #[arg(short, long, default_value = "table")]
        format: String,
    },

    /// Show details of a specific backup
    Info {
        /// Path to the backup directory
        #[arg(short, long)]
        backup_dir: PathBuf,

        /// Backup ID to show
        #[arg(short = 'i', long)]
        backup_id: String,
    },

    /// Restore a backup
    Restore {
        /// Path to the backup directory
        #[arg(short, long)]
        backup_dir: PathBuf,

        /// Backup ID to restore
        #[arg(short = 'i', long)]
        backup_id: String,

        /// Target path for restoration
        #[arg(short, long)]
        target: PathBuf,

        /// Overwrite target if it exists
        #[arg(long, default_value = "false")]
        overwrite: bool,
    },

    /// Delete a backup
    Delete {
        /// Path to the backup directory
        #[arg(short, long)]
        backup_dir: PathBuf,

        /// Backup ID to delete
        #[arg(short = 'i', long)]
        backup_id: String,

        /// Skip confirmation prompt
        #[arg(short, long, default_value = "false")]
        force: bool,
    },

    /// Cleanup old backups, keeping only the most recent N
    Cleanup {
        /// Path to the backup directory
        #[arg(short, long)]
        backup_dir: PathBuf,

        /// Number of backups to keep
        #[arg(short, long)]
        keep: usize,

        /// Skip confirmation prompt
        #[arg(short, long, default_value = "false")]
        force: bool,
    },

    /// Verify backup integrity
    Verify {
        /// Path to the backup directory
        #[arg(short, long)]
        backup_dir: PathBuf,

        /// Backup ID to verify (if not specified, verify all)
        #[arg(short = 'i', long)]
        backup_id: Option<String>,
    },
}

fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("byoridb_kvstore=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Create {
            db,
            backup_dir,
            label,
            no_flush,
        } => cmd_create(db, backup_dir, label, !no_flush),

        Commands::List { backup_dir, format } => cmd_list(backup_dir, format),

        Commands::Info {
            backup_dir,
            backup_id,
        } => cmd_info(backup_dir, backup_id),

        Commands::Restore {
            backup_dir,
            backup_id,
            target,
            overwrite,
        } => cmd_restore(backup_dir, backup_id, target, overwrite),

        Commands::Delete {
            backup_dir,
            backup_id,
            force,
        } => cmd_delete(backup_dir, backup_id, force),

        Commands::Cleanup {
            backup_dir,
            keep,
            force,
        } => cmd_cleanup(backup_dir, keep, force),

        Commands::Verify {
            backup_dir,
            backup_id,
        } => cmd_verify(backup_dir, backup_id),
    }
}

fn cmd_create(
    db_path: PathBuf,
    backup_dir: PathBuf,
    label: Option<String>,
    flush_before_backup: bool,
) -> Result<()> {
    println!("Creating backup...");
    println!("  Source: {:?}", db_path);
    println!("  Backup directory: {:?}", backup_dir);

    if !db_path.exists() {
        anyhow::bail!("Database path does not exist: {:?}", db_path);
    }

    let manager =
        BackupManager::new(&db_path, &backup_dir).context("Failed to create backup manager")?;

    let options = BackupOptions {
        label,
        flush_before_backup,
    };

    let backup_info = manager
        .create_backup(Some(options))
        .context("Failed to create backup")?;

    println!("\nBackup created successfully!");
    println!("  ID: {}", backup_info.id);
    println!("  Size: {}", format_bytes(backup_info.size_bytes));
    println!("  Created: {}", format_timestamp(backup_info.created_at));
    if let Some(label) = &backup_info.label {
        println!("  Label: {}", label);
    }

    Ok(())
}

fn cmd_list(backup_dir: PathBuf, format: String) -> Result<()> {
    if !backup_dir.exists() {
        println!("No backups found (backup directory does not exist)");
        return Ok(());
    }

    // We need a dummy db_path for BackupManager, but list only needs backup_dir
    let manager =
        BackupManager::new(&backup_dir, &backup_dir).context("Failed to create backup manager")?;

    let backups = manager.list_backups().context("Failed to list backups")?;

    if backups.is_empty() {
        println!("No backups found");
        return Ok(());
    }

    match format.as_str() {
        "json" => {
            let json = serde_json::to_string_pretty(&backups)?;
            println!("{}", json);
        }
        _ => {
            // Table format
            println!("{:<25} {:<25} {:<12} Label", "ID", "Created", "Size");
            println!("{}", "-".repeat(80));

            for backup in backups {
                println!(
                    "{:<25} {:<25} {:<12} {}",
                    backup.id,
                    format_timestamp(backup.created_at),
                    format_bytes(backup.size_bytes),
                    backup.label.unwrap_or_default()
                );
            }
        }
    }

    Ok(())
}

fn cmd_info(backup_dir: PathBuf, backup_id: String) -> Result<()> {
    let manager =
        BackupManager::new(&backup_dir, &backup_dir).context("Failed to create backup manager")?;

    let backup = manager
        .get_backup(&backup_id)
        .context("Failed to get backup info")?;

    println!("Backup Information");
    println!("{}", "=".repeat(40));
    println!("ID:          {}", backup.id);
    println!("Created:     {}", format_timestamp(backup.created_at));
    println!("Size:        {}", format_bytes(backup.size_bytes));
    println!("Source:      {}", backup.source_path);
    if let Some(label) = &backup.label {
        println!("Label:       {}", label);
    }

    Ok(())
}

fn cmd_restore(
    backup_dir: PathBuf,
    backup_id: String,
    target: PathBuf,
    overwrite: bool,
) -> Result<()> {
    println!("Restoring backup...");
    println!("  Backup ID: {}", backup_id);
    println!("  Target: {:?}", target);

    if target.exists() && !overwrite {
        anyhow::bail!(
            "Target path already exists: {:?}\nUse --overwrite to replace it",
            target
        );
    }

    let manager =
        BackupManager::new(&backup_dir, &backup_dir).context("Failed to create backup manager")?;

    manager
        .restore_backup(&backup_id, &target, overwrite)
        .context("Failed to restore backup")?;

    println!("\nBackup restored successfully to {:?}", target);

    Ok(())
}

fn cmd_delete(backup_dir: PathBuf, backup_id: String, force: bool) -> Result<()> {
    let manager =
        BackupManager::new(&backup_dir, &backup_dir).context("Failed to create backup manager")?;

    // Get backup info first
    let backup = manager
        .get_backup(&backup_id)
        .context("Failed to get backup info")?;

    if !force {
        println!("Are you sure you want to delete backup '{}'?", backup_id);
        println!("  Created: {}", format_timestamp(backup.created_at));
        println!("  Size: {}", format_bytes(backup.size_bytes));
        println!("\nType 'yes' to confirm: ");

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;

        if input.trim().to_lowercase() != "yes" {
            println!("Deletion cancelled");
            return Ok(());
        }
    }

    manager
        .delete_backup(&backup_id)
        .context("Failed to delete backup")?;

    println!("Backup '{}' deleted successfully", backup_id);

    Ok(())
}

fn cmd_cleanup(backup_dir: PathBuf, keep: usize, force: bool) -> Result<()> {
    let manager =
        BackupManager::new(&backup_dir, &backup_dir).context("Failed to create backup manager")?;

    let backups = manager.list_backups().context("Failed to list backups")?;

    if backups.len() <= keep {
        println!(
            "No cleanup needed. {} backups exist, keeping {}",
            backups.len(),
            keep
        );
        return Ok(());
    }

    let to_delete: Vec<_> = backups.into_iter().skip(keep).collect();

    if !force {
        println!(
            "The following {} backups will be deleted (keeping {} most recent):",
            to_delete.len(),
            keep
        );
        for backup in &to_delete {
            println!(
                "  - {} ({})",
                backup.id,
                format_timestamp(backup.created_at)
            );
        }
        println!("\nType 'yes' to confirm: ");

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;

        if input.trim().to_lowercase() != "yes" {
            println!("Cleanup cancelled");
            return Ok(());
        }
    }

    let deleted = manager
        .cleanup_old_backups(keep)
        .context("Failed to cleanup backups")?;

    println!("{} backups deleted successfully", deleted.len());

    Ok(())
}

fn cmd_verify(backup_dir: PathBuf, backup_id: Option<String>) -> Result<()> {
    let manager =
        BackupManager::new(&backup_dir, &backup_dir).context("Failed to create backup manager")?;

    let backups = if let Some(id) = backup_id {
        vec![manager.get_backup(&id).context("Failed to get backup")?]
    } else {
        manager.list_backups().context("Failed to list backups")?
    };

    if backups.is_empty() {
        println!("No backups found to verify");
        return Ok(());
    }

    println!("Verifying {} backup(s)...\n", backups.len());

    let mut success_count = 0;
    let mut fail_count = 0;

    for backup in backups {
        print!("Verifying {}... ", backup.id);

        // Try to get the backup info (which includes validation)
        match manager.get_backup(&backup.id) {
            Ok(_) => {
                println!("OK");
                success_count += 1;
            }
            Err(e) => {
                println!("FAILED: {}", e);
                fail_count += 1;
            }
        }
    }

    println!(
        "\nVerification complete: {} OK, {} FAILED",
        success_count, fail_count
    );

    if fail_count > 0 {
        anyhow::bail!("{} backup(s) failed verification", fail_count);
    }

    Ok(())
}
