// Incorpora a identidade visual do TsCIDE ao executavel gerado para Windows.
fn main() {
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed=../../resources/tscide.ico");
        let mut recursos = winres::WindowsResource::new();
        recursos.set_icon("../../resources/tscide.ico");
        recursos
            .compile()
            .expect("nao foi possivel incorporar o icone do TsCIDE");
    }
}
