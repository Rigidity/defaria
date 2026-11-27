-- Script-level state lasts between calls, until the scripts reload.
local count = 0

function on_tick(dt: number): number
	count += 1
	log.info("tick", count, dt)
	return count
end

-- Logs through `pcall`, a library function between the log and the script.
function on_log_caught()
	pcall(log.info, "caught")
end

-- Logs a huge string, many times over.
function on_log_huge()
	local s = string.rep("x", 1000000)
	for i = 1, 300 do
		log.info(s)
	end
end

-- Raises an error with a huge message.
function on_error_huge()
	error(string.rep("x", 1000000))
end
