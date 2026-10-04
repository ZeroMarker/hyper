# 模型完成状态与失败诊断

更新：2026-10-04。此规则适用于 agent loop 的 `chat_messages_stream`，包括服务端返回普通 JSON 的兼容路径。独立非流式 `chat_messages` 尚未应用相同策略。

Chat 累积工具参数，收到 `[DONE]` 后仍检查 `finish_reason`。`length` 或 `content_filter` 拒绝整个回复，即使工具参数已是完整 JSON，也不执行该批工具。缺失或未知结束原因保留为未知，兼容旧网关；SSE 缺少完成标记仍失败。Chat SSE 仅接受 choice 0，遇到其他明确索引拒绝，不合并多个候选。

Responses 仅以 `response.completed` 取得最终权威输出；`response.incomplete`、`response.failed`、`error` 均失败。普通 JSON 或 completed 事件中的显式非 completed 状态、非空 incomplete_details 也拒绝。未提供 status 的旧网关继续兼容。Messages 仍使用 `message_stop` 与参数完整性检查，记录 stop_reason；其停止原因的拒绝策略尚待单独核对和验收。Messages 缺失输入或输出计数时 usage 为 null，不补零；计数相加溢出也标未知。

所有工具调用在返回回复前统一校验：id/name 非空，参数必须为完整 JSON 对象。一条无效调用会使整个批次失败，邻接的有效调用也不会执行。已经发布的文本 delta 留在审计事件中；完成失败不生成 model.iteration/model.finished，也不自动重试该请求。请求开始前的现有限流/HTTP 重试保持原行为，取消和文本事件发布错误不改报模型完成错误。

`model.failed` 记录 turn、kind、completion；随后 step/run failure 的 errorType 为 `ModelCompletionError`、retryable=false，details 中保留相同 completion 及 completionFailureKind。completion 包含协议、sse/json、是否收到终结、允许列表映射的结束原因、SSE 帧数、发布文本 UTF-8 字节数、调用总数和收到的完整 usage。tools 最多记录 16 项，仅保留索引、参数字节/片段数、是否有 id/name。Chat 索引为流中 tool index，Messages 为 block index，Responses 为最终 output item index；JSON 兼容路径为调用顺序。argumentError.index 为返回调用列表中的序号，并记录 JSON 错误类别、行、列。完整 JSON/Responses 权威参数计作一次观察，不能据此推断网关实际发送的 delta 数量。

诊断不包含参数正文、工具名/id、provider error 正文或任意结束原因字符串；未知原因映射为 other。它描述解析器观察到的内容，不能单凭 EOF、片段数或 length 判断是谁造成了输出问题。普通模型文本、成功工具事件仍遵循已有审计策略，这里不宣称整个日志经过脱敏。

评测保存 completion_failures，但仅使用接受回复的 model.iteration 聚合用量。失败响应 reportedUsage 单独留作诊断，不纳入完整总量；未接受的最终回复仍使 usage_complete=false、总 usage=null，保留此前已知部分。未报告计数、计费价格与成本继续为未知；此次没有重算历史报告。

本地验证覆盖显式截断/过滤、Responses 未完成及矛盾终结、普通 JSON、缺终结、交错工具分片、整个批次拒绝、Messages 缺计数与 block 索引、16 项上限及参数正文不进入诊断。真实 CLI 验证普通 JSON/SSE 中完整 write 参数在 length 下不执行、不重试，stdout 与持久化事件一致；残缺最终回复保留此前用量与新的失败诊断。已有取消、背压及事件发布失败测试继续覆盖运行边界。

完成状态的依据于本日核对 [OpenAI Chat API](https://developers.openai.com/api/reference/resources/chat) 与 [Responses streaming events](https://developers.openai.com/api/reference/resources/responses/streaming-events)。兼容网关及其他协议不能仅据 OpenAI 文档宣称符合；provider/model 能力配置、tokenizer、非流式统一策略、Messages 停止语义及重复调用阈值继续列在 P1-5。
