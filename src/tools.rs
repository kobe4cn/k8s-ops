use rig::{completion::{Chat, Completion, Prompt}, providers::{self, deepseek::DEEPSEEK_CHAT}, tool::Tool};
use serde::{Deserialize, Serialize};
use serde_json::json;

//生成k8s YAML 并部署资源
#[derive(Serialize, Deserialize)]
pub struct GenerateAndDeployResource;

#[derive(Deserialize)]
// pub struct OperationArgs {
//     user_input: String,
// }

//查询k8s资源
#[derive(Serialize, Deserialize)]
struct QueryResource;

#[derive(Debug, thiserror::Error)]
#[error("error: {0}")]
pub enum DeployError {
    #[error("Prompt error: {0}")]
    PromptError(#[from] rig::completion::PromptError),
    #[error("Completion error: {0}")]
    CompletionError(#[from] rig::completion::CompletionError),
}

impl Tool for GenerateAndDeployResource {
    const NAME: &'static str = "generate_and_deploy_resource";
    type Error = DeployError;
    type Args = OperationArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> rig::completion::ToolDefinition {
        serde_json::from_value(json!(
            {
                "name": Self::NAME,
                "description": "生成 K8S YAML 并部署资源",
                "parameters":
                    {
                        "type": "object",
                        "properties":
                            {
                                "user_input":
                                    {
                                        "type": "string",
                                        "description": "用户输出的文本内容，要求包含资源类型和镜像"
                                    },
                            }
                    }
            }
        ))
        .expect("YAML文件生成部署工具失败")
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let client = providers::deepseek::Client::from_env();
        let agent=client.agent(DEEPSEEK_CHAT)
            .preamble("你现在是一个 K8s 资源生成器，根据用户输入生成 K8s YAML，注意除了 YAML 内容以外不要输出任何内容，此外不要把 YAML 放在 ``` 代码快里")
            .max_tokens(1024).tool(GenerateAndDeployResource)
            .build();

        let resp=agent.prompt(args.user_input).await?;
        Ok("".to_string())

    }
}
