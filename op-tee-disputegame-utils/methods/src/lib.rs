// RISC Zero 方法定义和导出

// 导出 ELF 和 ID 常量供 Host 使用
include!(concat!(env!("OUT_DIR"), "/methods.rs"));
