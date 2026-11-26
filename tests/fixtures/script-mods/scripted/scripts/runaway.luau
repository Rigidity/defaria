function on_tick()
	while true do
	end
end

-- Catching the budget's error doesn't get past it.
function on_catch()
	while true do
		pcall(function()
			while true do
			end
		end)
	end
end

-- Runs out inside a library function: a pattern that backtracks.
function on_find()
	return string.rep("a", 3000):find(string.rep("a-", 6) .. "b")
end

-- Few steps, but each one a long library call: stopped by the time limit.
function on_sort()
	local big = table.create(200000, 0)
	for i = 1, 100000 do
		table.sort(big, nil)
	end
end
