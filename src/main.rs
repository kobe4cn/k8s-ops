use futures::TryStreamExt;
use k8s_openapi::api::core::v1::Event;
use kube::{
    api::Api,
    runtime::{watcher, WatchStreamExt},
    Client,
};
#[tokio::main]
async fn main() -> Result<(), watcher::Error> {
    let client = Client::try_default().await.unwrap();
    let pods: Api<Event> = Api::namespaced(client, "default");

    watcher(pods, watcher::Config::default())
        .applied_objects()
        .try_for_each(|p| async move {
            if p.type_ == Some("Warning".to_string())
                && p.involved_object.kind == Some("Pod".to_string())
            {
                println!(
                    "Warning: {:?} {:?} {:?} {:?}",
                    p.involved_object.name.unwrap(),
                    p.involved_object.namespace,
                    p.reason.unwrap(),
                    p.message.unwrap()
                );
            }
            Ok(())
        })
        .await?;
    Ok(())
}
