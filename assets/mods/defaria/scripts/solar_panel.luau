-- A solar panel (blocks/solar_panel.json5): makes energy from the sky
-- light on its top, `max_generate` (60 W) in open sky (sky light 15),
-- less as less sky reaches it, none with a block on it.
--
-- Its data keeps what it's made in all (`made`, in joules) and the
-- fraction of a joule it's owed toward the next tick (`carry`).

function on_tick(panel, dt)
	local above = panel:neighbour("up")
	local light = above and above.light
	local sky = if light then light.sky else 0
	-- Each handle field read asks the engine, so read each once.
	local energy, data = panel.energy, panel.data
	local owed = energy.max_generate * dt * sky / 15 + data.carry
	local whole = math.floor(owed)
	-- No more than there's room for: a full panel makes nothing.
	local made = energy:generate(whole)
	data.carry = owed - whole
	data.made += made
	panel:set_active(made > 0)
end
