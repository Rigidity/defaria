local kept = {}

function on_tick()
	for i = 1, 10000000 do
		kept[i] = i
	end
end
