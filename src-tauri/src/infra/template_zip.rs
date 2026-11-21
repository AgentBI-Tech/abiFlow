use std::io::{Cursor, Read, Write};

use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::models::{AppError, AppResult, TemplateBundle};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const SOP_FILE: &str = "sop.json";
pub const TRAJECTORY_FILE: &str = "trajectory.jsonl";
pub const CORRECTIONS_FILE: &str = "corrections.jsonl";
pub const README_FILE: &str = "README.txt";

/// zip 格式版本号：区别于 schema（SOP AST 结构版本）。只递增不影响旧 zip 导入（导入端忽略它）。
pub const MANIFEST_VERSION: i32 = 1;

/// 统计条目数：这两个文件实际存的是 JSON 数组（非真 jsonl），解析成数组取长度；非法/空取 0。
fn count_items(json: &str) -> usize {
    match serde_json::from_str::<serde_json::Value>(json) {
        Ok(serde_json::Value::Array(a)) => a.len(),
        _ => 0,
    }
}

/// 导出为 zip（§6）：manifest.json / sop.json / trajectory.jsonl / corrections.jsonl / README.txt。
/// manifest 除必需字段外附带导出时间、统计与格式版本（导入端忽略这些附加字段，向后兼容旧 zip）。
pub fn export_zip(bundle: &TemplateBundle) -> AppResult<Vec<u8>> {
    let manifest = serde_json::json!({
        "schema": bundle.manifest_schema,
        "name": bundle.name,
        "validated": bundle.validated,
        "source_shop": bundle.source_shop,
        "manifest_version": MANIFEST_VERSION,
        "exported_at": chrono::Utc::now().to_rfc3339(),
        "step_count": count_items(&bundle.trajectory_json),
        "correction_count": count_items(&bundle.corrections_json),
    });
    let manifest_s = serde_json::to_string_pretty(&manifest)
        .map_err(|e| AppError::InvalidInput(e.to_string()))?;
    let readme = format!(
        "abiFlow 经验模板\n名称：{}\n验证通过：{}\n来源店铺：{}\n\nschema={}\nmanifest_version={}\n步骤数={}\n修正数={}\n",
        bundle.name,
        if bundle.validated { "是" } else { "否" },
        bundle.source_shop.as_deref().unwrap_or("未标注"),
        bundle.manifest_schema,
        MANIFEST_VERSION,
        count_items(&bundle.trajectory_json),
        count_items(&bundle.corrections_json),
    );

    let mut buf = Vec::new();
    {
        let mut zw = ZipWriter::new(Cursor::new(&mut buf));
        let opts = SimpleFileOptions::default();
        for (name, content) in [
            (MANIFEST_FILE, manifest_s.as_str()),
            (SOP_FILE, bundle.sop_snapshot.as_str()),
            (TRAJECTORY_FILE, bundle.trajectory_json.as_str()),
            (CORRECTIONS_FILE, bundle.corrections_json.as_str()),
            (README_FILE, readme.as_str()),
        ] {
            zw.start_file(name, opts)
                .map_err(|e| AppError::Db(format!("zip 写入 {} 失败：{}", name, e)))?;
            zw.write_all(content.as_bytes())
                .map_err(|e| AppError::Db(format!("zip 写入 {} 内容失败：{}", name, e)))?;
        }
        zw.finish().map_err(|e| AppError::Db(format!("zip 收尾失败：{}", e)))?;
    }
    Ok(buf)
}

/// 导入：从 zip 读出 bundle。缺文件或非法 JSON 报错；validated/schema 由 domain 校验。
/// 只读必需字段（schema/name/validated/source_shop）；manifest_version/exported_at/step_count/
/// correction_count 等附加字段被忽略——保证旧 zip（无这些字段）可正常导入，向后兼容。
pub fn import_zip(bytes: &[u8]) -> AppResult<TemplateBundle> {
    let mut archive = ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| AppError::InvalidInput(format!("zip 解析失败：{}", e)))?;
    let mut read = |name: &str| -> AppResult<String> {
        let mut entry = archive
            .by_name(name)
            .map_err(|_| AppError::InvalidInput(format!("模板缺文件：{}", name)))?;
        let mut s = String::new();
        entry
            .read_to_string(&mut s)
            .map_err(|e| AppError::InvalidInput(format!("读取 {} 失败：{}", name, e)))?;
        Ok(s)
    };

    let manifest_s = read(MANIFEST_FILE)?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest_s)
        .map_err(|e| AppError::InvalidInput(format!("manifest 非法：{}", e)))?;
    let schema = manifest
        .get("schema")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| AppError::InvalidInput("manifest 缺 schema".into()))? as i32;
    let name = manifest
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("未命名")
        .to_string();
    let validated = manifest.get("validated").and_then(|v| v.as_bool()).unwrap_or(false);
    let source_shop = manifest
        .get("source_shop")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(TemplateBundle {
        manifest_schema: schema,
        name,
        validated,
        sop_snapshot: read(SOP_FILE)?,
        trajectory_json: read(TRAJECTORY_FILE)?,
        corrections_json: read(CORRECTIONS_FILE)?,
        source_shop,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> TemplateBundle {
        TemplateBundle {
            manifest_schema: 1,
            name: "巡检模板".into(),
            validated: true,
            sop_snapshot: "{\"version\":1}".into(),
            trajectory_json: "[{\"nodeType\":\"mcp_tool\"}]".into(),
            corrections_json: "[]".into(),
            source_shop: Some("shop-1".into()),
        }
    }

    #[test]
    fn zip_roundtrip() {
        let bytes = export_zip(&bundle()).unwrap();
        assert!(bytes.len() > 20);
        let back = import_zip(&bytes).unwrap();
        assert_eq!(back.name, "巡检模板");
        assert!(back.validated);
        assert_eq!(back.sop_snapshot, "{\"version\":1}");
        assert_eq!(back.source_shop.as_deref(), Some("shop-1"));
    }

    #[test]
    fn zip_has_all_five_files() {
        let bytes = export_zip(&bundle()).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        for n in [MANIFEST_FILE, SOP_FILE, TRAJECTORY_FILE, CORRECTIONS_FILE, README_FILE] {
            assert!(archive.by_name(n).is_ok(), "缺 {}", n);
        }
    }

    #[test]
    fn manifest_has_extra_fields_and_counts() {
        let bytes = export_zip(&bundle()).unwrap();
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut s = String::new();
        archive.by_name(MANIFEST_FILE).unwrap().read_to_string(&mut s).unwrap();
        let m: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(m["manifest_version"], MANIFEST_VERSION as i64);
        assert!(m["exported_at"].as_str().map(|x| !x.is_empty()).unwrap_or(false), "缺 exported_at");
        // bundle() 的 trajectory 有 1 个元素、corrections 空数组。
        assert_eq!(m["step_count"], 1);
        assert_eq!(m["correction_count"], 0);
    }

    #[test]
    fn old_manifest_without_extra_fields_still_imports() {
        // 手工造一个只有旧字段的 zip，导入应成功（附加字段被忽略）。
        let mut buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            let o = SimpleFileOptions::default();
            let old = r#"{"schema":1,"name":"旧模板","validated":true,"source_shop":null}"#;
            for (n, c) in [
                (MANIFEST_FILE, old), (SOP_FILE, "{\"version\":1}"),
                (TRAJECTORY_FILE, "[]"), (CORRECTIONS_FILE, "[]"), (README_FILE, "old"),
            ] {
                zw.start_file(n, o).unwrap();
                zw.write_all(c.as_bytes()).unwrap();
            }
            zw.finish().unwrap();
        }
        let back = import_zip(&buf).unwrap();
        assert_eq!(back.name, "旧模板");
        assert!(back.validated);
    }

    #[test]
    fn import_rejects_missing_manifest() {
        let mut buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            zw.start_file(SOP_FILE, SimpleFileOptions::default()).unwrap();
            zw.write_all(b"{}").unwrap();
            zw.finish().unwrap();
        }
        assert!(import_zip(&buf).is_err());
    }
}
