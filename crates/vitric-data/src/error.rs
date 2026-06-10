use std::fmt;

use serde::Serialize;

/// 一条校验错误：路径精确到字段，带错误码和修复提示。
/// 序列化后直接进控制面响应——AI 拿到的就是这个结构。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ValidationError {
    /// 稳定错误码，如 "VD003"。文档按码索引。
    pub code: &'static str,
    /// 出错位置，如 "scenes/main.json#/entities/2/components/Sprite/image"。
    pub path: String,
    /// 哪里错了。
    pub message: String,
    /// 怎么修。
    pub hint: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}: {}（{}）", self.code, self.path, self.message, self.hint)
    }
}

/// 一次校验的全部结果。校验不在第一个错误就停——AI 一次拿到所有问题。
#[derive(Debug, Default, Clone, Serialize)]
pub struct ValidationReport {
    pub errors: Vec<ValidationError>,
}

impl ValidationReport {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn push(
        &mut self,
        code: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
        hint: impl Into<String>,
    ) {
        self.errors.push(ValidationError {
            code,
            path: path.into(),
            message: message.into(),
            hint: hint.into(),
        });
    }

    pub fn merge(&mut self, other: ValidationReport) {
        self.errors.extend(other.errors);
    }

    /// 转成 Err；没错误则给回传入值。
    pub fn into_result<T>(self, value: T) -> Result<T, ValidationReport> {
        if self.ok() {
            Ok(value)
        } else {
            Err(self)
        }
    }
}

impl fmt::Display for ValidationReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "校验发现 {} 个问题:", self.errors.len())?;
        for e in &self.errors {
            writeln!(f, "  {e}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ValidationReport {}
