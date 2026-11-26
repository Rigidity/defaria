-- A lamp with a switch (blocks/switch_lamp.json5).

-- Using it turns it on or off, and counts the switching in its data.
function on_used(block, player)
	block:set_state({ lit = not block.state.lit })
	block.data.switched += 1
end
