-- A clump of moss (items/moss.json5), and what moss grows on, for
-- mossy stone too.

local moss = {}

-- Bare stone (plain stone, no ore) with air on at least one side.
function moss.can_grow_on(block): boolean
	if block.id ~= "defaria:stone" or block.state.rock ~= "defaria:stone" or block.state.ore ~= "none" then
		return false
	end
	for _, face in world.faces do
		local next = block:neighbour(face)
		if next and next.id == "air" then
			return true
		end
	end
	return false
end

-- Used on bare stone, it makes the stone mossy and is used up.
function on_use(item, player)
	local target = player.target
	if target and moss.can_grow_on(target) then
		return target:set("defaria:mossy_stone")
	end
	return false
end

return moss
