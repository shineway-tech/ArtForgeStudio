//! ArtAIT 旧版数据迁移 CLI 工具。
//!
//! 用法：
//! ```sh
//! artait-migrate.exe scan "D:\BoBO\ArtAIT"
//! artait-migrate.exe generate "D:\BoBO\ArtAIT"
//! artait-migrate.exe dry-run "D:\BoBO\ArtAIT"
//! ```
//!
//! `scan`：扫描旧目录，输出迁移报告（不写文件）。
//! `generate`：生成 `app_config.toml` 到 `%APPDATA%\ArtAIT\`。
//! `dry-run`：展示将要生成的配置内容（不写入）。

use std::path::PathBuf;

use anyhow::Result;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法: artait-migrate <scan|generate|dry-run> <旧版目录路径>");
        eprintln!();
        eprintln!("  scan      扫描旧目录，输出迁移报告");
        eprintln!("  generate  生成 app_config.toml 到配置目录");
        eprintln!("  dry-run   展示将要生成的 TOML 内容");
        std::process::exit(1);
    }

    let command = &args[1];
    let legacy_dir = PathBuf::from(&args[2]);

    if !legacy_dir.exists() {
        eprintln!("错误：目录不存在 → {}", legacy_dir.display());
        std::process::exit(1);
    }

    let config_json = legacy_dir.join("config.json");
    if !config_json.exists() {
        eprintln!("警告：未找到 config.json，将只扫描目录结构");
    }

    match command.as_str() {
        "scan" => cmd_scan(&legacy_dir)?,
        "generate" => cmd_generate(&legacy_dir)?,
        "dry-run" => cmd_dry_run(&legacy_dir)?,
        _ => {
            eprintln!("未知命令: {command}");
            std::process::exit(1);
        }
    }
    Ok(())
}

fn cmd_scan(legacy_dir: &PathBuf) -> Result<()> {
    println!("=== ArtAIT 旧版数据扫描 ===");
    println!("目录: {}", legacy_dir.display());
    println!();

    let mut total_files = 0usize;
    let mut total_size = 0u64;

    // 扫描 out/
    let out_dir = legacy_dir.join("out");
    if out_dir.exists() {
        let (count, size) = count_files(&out_dir);
        println!("out/         {} 个文件 · {} MB", count, size / 1024 / 1024);
        total_files += count;
        total_size += size;
    } else {
        println!("out/         （不存在）");
    }

    // 扫描 prompt/
    let prompt_dir = legacy_dir.join("prompt");
    if prompt_dir.exists() {
        let (count, size) = count_files(&prompt_dir);
        println!("prompt/      {} 个文件 · {} KB", count, size / 1024);
        total_files += count;
        total_size += size;
    } else {
        println!("prompt/      （不存在）");
    }

    // 扫描 apply_prompt/
    let apply_dir = legacy_dir.join("apply_prompt");
    if apply_dir.exists() {
        let (count, size) = count_files(&apply_dir);
        println!("apply_prompt/ {} 个文件 · {} MB", count, size / 1024 / 1024);
        total_files += count;
        total_size += size;
    } else {
        println!("apply_prompt/ （不存在）");
    }

    // 扫描 input/
    let input_dir = legacy_dir.join("input");
    if input_dir.exists() {
        let (count, size) = count_files(&input_dir);
        println!("input/       {} 个文件 · {} KB", count, size / 1024);
        total_files += count;
        total_size += size;
    } else {
        println!("input/       （不存在）");
    }

    // config.json
    let config_json = legacy_dir.join("config.json");
    if config_json.exists() {
        match artait_config::migrate_from_legacy_json(&config_json) {
            Ok((_cfg, report)) => {
                println!();
                println!("=== config.json 迁移报告 ===");
                println!(
                    "provider 实例: 发现 {} · 导入 {}",
                    report.providers_found, report.providers_imported
                );
                println!(
                    "路径导入: {}",
                    if report.paths_imported { "是" } else { "否" }
                );
                if !report.warnings.is_empty() {
                    println!("警告:");
                    for w in &report.warnings {
                        println!("  - {w}");
                    }
                }
                if !report.secret_keys_seen.is_empty() {
                    println!("密钥引用（不含密钥值）:");
                    for k in &report.secret_keys_seen {
                        println!("  - {k}");
                    }
                }
            }
            Err(e) => {
                println!("config.json 解析失败: {e}");
            }
        }
    }

    println!();
    println!("=== 总计 ===");
    println!("文件数: {total_files}");
    println!("总大小: {} MB", total_size / 1024 / 1024);
    println!();
    // 扫描旧 prompt 模板
    let prompt_dir = legacy_dir.join("prompt");
    if prompt_dir.exists() {
        println!();
        println!("=== 旧 prompt 模板 ===");
        let template_dirs = [
            ("create_character_prompt", "角色提示词"),
            ("create_scene_prompt", "场景提示词"),
            ("create_ui_prompt", "UI提示词"),
            ("create_effect_prompt", "特效提示词"),
        ];
        for (sub, label) in &template_dirs {
            let d = prompt_dir.join(sub);
            if d.exists() {
                let (count, _size) = count_files(&d);
                if count > 0 {
                    println!("  {label}: {count} 个模板");
                }
            }
        }
    }

    println!();
    println!("迁移建议:");
    println!(
        "  1. 运行 'artait-migrate generate {}' 生成新配置",
        legacy_dir.display()
    );
    println!("  2. 启动 ArtForgeStudio，图库会自动索引输出目录中的资产");
    println!("  3. 在设置页重新配置 API Key（旧密钥不会自动导入）");
    println!("  4. 旧 prompt/ 模板可手动复制到新 prompt_dir/ 对应子目录");

    Ok(())
}

fn cmd_dry_run(legacy_dir: &PathBuf) -> Result<()> {
    let config_json = legacy_dir.join("config.json");
    if !config_json.exists() {
        eprintln!("未找到 config.json，无法生成配置");
        std::process::exit(1);
    }

    match artait_config::migrate_from_legacy_json(&config_json) {
        Ok((cfg, report)) => {
            println!("=== 迁移预览（不会写入） ===");
            println!();
            println!(
                "provider 实例: 发现 {} · 导入 {}",
                report.providers_found, report.providers_imported
            );
            if !report.warnings.is_empty() {
                println!("警告: {:?}", report.warnings);
            }
            println!();
            println!("=== 生成的 app_config.toml ===");
            match toml::to_string_pretty(&cfg) {
                Ok(s) => println!("{s}"),
                Err(e) => eprintln!("序列化失败: {e}"),
            }
        }
        Err(e) => {
            eprintln!("config.json 解析失败: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}

fn cmd_generate(legacy_dir: &PathBuf) -> Result<()> {
    let config_json = legacy_dir.join("config.json");
    let (cfg, _report) = if config_json.exists() {
        artait_config::migrate_from_legacy_json(&config_json)?
    } else {
        let mut cfg = artait_model::AppConfig::default();
        cfg.paths.input_dir = legacy_dir.join("input");
        cfg.paths.output_dir = legacy_dir.join("out");
        cfg.paths.prompt_dir = legacy_dir.join("prompt");
        cfg.paths.apply_prompt_dir = legacy_dir.join("apply_prompt");
        cfg.migrated_from = Some(legacy_dir.clone());
        (cfg, artait_config::MigrationReport::default())
    };

    artait_config::save(&cfg)?;
    let path = artait_config::app_config_path()?;
    println!("配置已保存到 {}", path.display());
    println!();
    println!("下一步:");
    println!("  1. 启动 ArtForgeStudio.exe");
    println!("  2. 在设置页配置 API Key（旧密钥不会自动导入）");
    Ok(())
}

fn count_files(dir: &PathBuf) -> (usize, u64) {
    let mut count = 0usize;
    let mut size = 0u64;
    fn walk(dir: &std::path::Path, count: &mut usize, size: &mut u64) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    walk(&p, count, size);
                } else if let Ok(meta) = p.metadata() {
                    *count += 1;
                    *size += meta.len();
                }
            }
        }
    }
    walk(dir, &mut count, &mut size);
    (count, size)
}
