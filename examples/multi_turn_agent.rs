use std::sync::Arc;

use kube::{
    api::{Api, ApiResource, DynamicObject, GroupVersionKind, PostParams},
    Client,
};
use rig::{
    agent::Agent,
    completion::{self, Completion, PromptError, ToolDefinition},
    message::{AssistantContent, Message, ToolCall, ToolFunction, ToolResultContent, UserContent},
    providers::anthropic,
    tool::Tool,
    OneOrMany,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

struct MultiTurnAgent<M: rig::completion::CompletionModel> {
    agent: Agent<M>,
    chat_history: Vec<completion::Message>,
}

impl<M: rig::completion::CompletionModel> MultiTurnAgent<M> {
    async fn multi_turn_prompt(
        &mut self,
        prompt: impl Into<Message> + Send,
    ) -> Result<String, PromptError> {
        let mut current_prompt: Message = prompt.into();
        loop {
            println!("Current Prompt: {:?}\n", current_prompt);
            // println!("Chat History: {:?}\n", self.chat_history);
            let resp = self
                .agent
                .completion(current_prompt.clone(), self.chat_history.clone())
                .await?
                .send()
                .await?;

            let mut final_text = None;
            if resp.choice.is_empty() {
                return Ok("执行完成".to_string());
            }
            for content in resp.choice.into_iter() {
                match content {
                    AssistantContent::Text(text) => {
                        println!("Intermediate Response: {:?}\n", text.text);
                        final_text = Some(text.text.clone());
                        self.chat_history.push(current_prompt.clone());
                        let response_message = Message::Assistant {
                            content: OneOrMany::one(AssistantContent::text(&text.text)),
                        };
                        self.chat_history.push(response_message);
                    }
                    AssistantContent::ToolCall(content) => {
                        self.chat_history.push(current_prompt.clone());
                        let tool_call_msg = AssistantContent::ToolCall(content.clone());
                        println!("Tool Call Msg: {:?}\n", tool_call_msg);

                        self.chat_history.push(Message::Assistant {
                            content: OneOrMany::one(tool_call_msg),
                        });

                        let ToolCall {
                            id,
                            function: ToolFunction { name, arguments },
                        } = content;

                        let tool_result =
                            self.agent.tools.call(&name, arguments.to_string()).await?;

                        current_prompt = Message::User {
                            content: OneOrMany::one(UserContent::tool_result(
                                id,
                                OneOrMany::one(ToolResultContent::text(tool_result)),
                            )),
                        };

                        final_text = None;
                        break;
                    }
                }
            }

            if let Some(text) = final_text {
                return Ok(text);
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create OpenAI client
    let openai_client = anthropic::Client::from_env();

    // Create RAG agent with a single context prompt and a dynamic tool source
    let calculator_rag = openai_client
        .agent(anthropic::CLAUDE_3_5_SONNET)
        .preamble(
            "你现在是一个 K8s 资源生成器和执行器。
            1. 根据用户输入生成 K8s YAML，注意除了 YAML 内容以外不要输出任何内容，此外不要把 YAML 放在 ``` 代码快里。
            2. 根据上下文选择合适的工具执行。
            3. 不要执行操作，只是根据用户输入选择合适的工具执行。
            4. 如果获得成功信息，则返回成功信息，否则返回失败信息。
            "
        )
        .tool(ApplyYamlToK8s)
        .build();

    let mut agent = MultiTurnAgent {
        agent: calculator_rag,
        chat_history: Vec::new(),
    };

    // Prompt the agent and print the response
    let result = agent
        .multi_turn_prompt("帮我生成一个 deploy,  镜像是 nginx, 并执行部署操作。返回执行情况。")
        .await?;

    println!("\n\nAgent: {}", result);

    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    #[error("Prompt error: {0}")]
    PromptError(#[from] rig::completion::PromptError),
    #[error("Completion error: {0}")]
    CompletionError(#[from] rig::completion::CompletionError),
    #[error("Apply error: {0}")]
    ApplyError(#[from] anyhow::Error),
    #[error("Boxed error: {0}")]
    BoxedError(#[from] Box<dyn std::error::Error + Send + Sync>),
    #[error("Serde error: {0}")]
    SerdeError(#[from] serde_json::Error),
    #[error("Serde error: {0}")]
    SerdeYamlError(#[from] serde_yaml::Error),
    #[error("Kube error: {0}")]
    KubeError(#[from] kube::Error),
}

//生成k8s YAML 并部署资源
#[derive(Serialize, Deserialize, Clone)]
struct ApplyYamlToK8s;

#[derive(Deserialize, Clone, Serialize)]
struct K8sArg {
    user_input: String,
}
impl Tool for ApplyYamlToK8s {
    const NAME: &'static str = "apply_yaml_to_k8s";
    type Error = ApplyError;
    type Args = K8sArg;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        serde_json::from_value(json!(
            {
                "name": Self::NAME,
                "description": "基于YAML文件执行K8S资源部署",
                "parameters":
                    {
                        "type": "object",
                        "properties":
                            {
                                "user_input":
                                    {
                                        "type": "string",
                                        "description": "yaml文件内容"
                                    },
                            }
                    }
            }
        ))
        .expect("执行失败")
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        println!("执行K8S资源部署{}", args.user_input);
        let yaml_content = Arc::new(args.user_input);
        let resp = self.apply_yaml_to_k8s(&yaml_content).await?;
        Ok(resp)
    }

    fn name(&self) -> String {
        Self::NAME.to_string()
    }
}

impl ApplyYamlToK8s {
    async fn apply_yaml_to_k8s(&self, yaml_str: &Arc<String>) -> Result<String, ApplyError> {
        let yaml_str_clone = Arc::clone(yaml_str);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            let result = rt.block_on(async {
                let client = Client::try_default().await?;
                let obj: DynamicObject = serde_yaml::from_str(&yaml_str_clone)?;
                // API版本和类型信息
                // let api_version = obj
                //     .types
                //     .as_ref()
                //     .and_then(|t| Some(t.api_version.clone()))
                //     .ok_or_else(|| anyhow::anyhow!("API版本信息缺失"))?;
                let api_version = obj
                    .types
                    .as_ref()
                    .map(|t| t.api_version.clone())
                    .ok_or_else(|| anyhow::anyhow!("API版本信息缺失"))?;

                // let kind = obj
                //     .types
                //     .as_ref()
                //     .and_then(|t| Some(t.kind.clone()))
                //     .ok_or_else(|| anyhow::anyhow!("类型信息缺失"))?;

                let kind = obj
                    .types
                    .as_ref()
                    .map(|t| t.kind.clone())
                    .ok_or_else(|| anyhow::anyhow!("类型信息缺失"))?;

                // 解析 API 版本为 (group, version)
                let (group, version) = parse_api_version(api_version.as_str());
                let gvk = GroupVersionKind::gvk(group.unwrap(), version, kind.as_str());
                // 获取命名空间（默认为 default）
                let namespace = obj.metadata.namespace.as_deref().unwrap_or("default");

                // 创建对应的 API 资源
                let api_resource = ApiResource::from_gvk(&gvk);
                // 创建对应的 API 接口
                let api: Api<DynamicObject> =
                    Api::namespaced_with(client.clone(), namespace, &api_resource);

                // 创建 Kubernetes 资源
                let pp = PostParams::default();
                api.create(&pp, &obj).await?;
                // let res = format!(
                //     " {}: {} in namespace {}",
                //     kind,
                //     obj.name_any(),
                //     namespace
                // );
                let res = "部署完成".to_string();
                println!("{}", res);

                Ok(res)
            });
            tx.send(result).unwrap();
        });
        rx.recv().unwrap()
    }
}

fn parse_api_version(api_version: &str) -> (Option<&str>, &str) {
    api_version
        .split_once('/')
        .map_or((None, api_version), |(g, v)| (Some(g), v))
}
