-- Mossy stone (blocks/mossy_stone.json5): moss creeps onto bare stone.

local moss = require("moss")

-- On a random tick, it tries one side: bare stone there that's open to
-- the air turns mossy.
function on_random_tick(block)
	local face = world.faces[math.random(#world.faces)]
	local next = block:neighbour(face)
	if next and moss.can_grow_on(next) then
		next:set("defaria:mossy_stone")
	end
end
