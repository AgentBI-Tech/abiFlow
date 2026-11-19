use std::io::{Cursor, Read, Write};

use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::models::{AppError, AppResult, TemplateBundle};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const SOP_FILE: &str = "sop.json";
pub const TRAJECTORY_FILE: &str = "trajectory.jsonl";
pub const CORRECTIONS_FILE: &str = "corrections.jsonl";
pub const README_FILE: &str = "README.txt";

/// 导出为 zip（§6）：manifest.json / sop.json / trajectory.jsonl / corrections.jsonl / README.txt。
pub fn export_zip(bundle: &TemplateBundle) -> AppResult<Vec<u8>> {
    let manifest = serde_json::json!({
        "schema": bundle.manifest_schema,
        "name": bundle.name,
        "validated": bundle.validated,
        "source_shop": bundle.source_shop,
    });
    let manifest_s = serde_json::to_string_pretty(&manifest)
        .map_err(|e| AppError::InvalidInput(e.to_string()))?;
    let readme = format!(
        "ABI-Pilot 经验模板\n名称：{}\n验证通过：{}\n来源店铺：{}\n\nschema={}\n",
        bundle.name,
        if bundle.validated { "是" } else { "否" },
        bundle.source_shop.as_deref().unwrap_or("未标注"),
        bundle.manifest_schema,
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
