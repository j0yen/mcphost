use crate::serve::{ServeChild, ServeEnv};
#[tokio::test]
async fn over_helper() {
    let mut server = ServeChild::spawn(ServeEnv::default());
    server.wait_ready().await;
    let _stderr = server.diagnostics();
}
