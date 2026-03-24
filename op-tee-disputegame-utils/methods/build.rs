use std::collections::HashMap;
use risc0_build::{DockerOptionsBuilder, GuestOptionsBuilder};

fn main() {
    // 配置 Docker 选项
    let docker_options = DockerOptionsBuilder::default()
        .root_dir("../../")
        .build()
        .unwrap();

    // 配置 Guest 程序选项，启用 Docker
    let guest_options = GuestOptionsBuilder::default()
        .use_docker(docker_options)
        .build()
        .unwrap();

    // 创建选项映射
    let options = HashMap::from([
        ("guest", guest_options)
    ]);
    
    // 嵌入 Guest 程序并生成必要的常量
    risc0_build::embed_methods_with_options(options);
    // risc0_build::embed_methods();

}
