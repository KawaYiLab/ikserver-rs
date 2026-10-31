-- 工具结果不再按固定字数截断（ai-assistant.md TOOL-002、TOOL-033）：放不下时由上下文预算处理（LLM-018），
-- 站外服务的结果上限随之去掉。
ALTER TABLE ai_mcp_servers DROP CONSTRAINT ai_mcp_servers_max_result;
ALTER TABLE ai_mcp_servers DROP COLUMN max_result_chars;
