return function(loom)
    loom.register_goal_strategy(function(ctx, facts)
        if facts.cancelled or facts.budget_exhausted or facts.unrecoverable or facts.completed then
            return {decision = "stop"}
        end
        if facts.waiting then
            return {decision = "wait"}
        end
        return {
            decision = "continue",
            instruction = "继续当前目标。先核对已有进度、完成条件和证据，再执行剩余工作。"
        }
    end)
end
