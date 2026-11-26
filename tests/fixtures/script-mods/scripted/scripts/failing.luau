-- Fails on line 4 whenever it's called.
function on_tick()
	local t = nil
	return t.x
end
