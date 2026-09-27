package dev.lookhin.flyminer;

import com.google.gson.Gson;
import com.google.gson.JsonObject;
import com.mojang.logging.LogUtils;
import net.minecraft.client.KeyMapping;
import net.minecraft.client.Minecraft;
import net.minecraft.commands.Commands;
import net.minecraft.commands.SharedSuggestionProvider;
import net.minecraft.commands.arguments.ResourceLocationArgument;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.network.chat.Component;
import net.minecraft.tags.BlockTags;
import net.minecraft.tags.ItemTags;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.entity.Mob;
import net.minecraft.world.entity.monster.Enemy;
import net.minecraft.world.inventory.ClickType;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.level.ClipContext;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.BlockHitResult;
import net.minecraft.world.phys.HitResult;
import net.minecraft.world.phys.Vec3;
import net.minecraftforge.client.event.RegisterClientCommandsEvent;
import net.minecraftforge.client.event.RenderGuiOverlayEvent;
import net.minecraftforge.fml.ModList;
import net.minecraftforge.common.MinecraftForge;
import net.minecraftforge.common.Tags;
import net.minecraftforge.event.TickEvent;
import net.minecraftforge.fml.loading.FMLPaths;
import org.slf4j.Logger;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Set;

/** Client-only local-file bridge. All world access and actions run on the client thread. */
public final class ClientBridge {
    private static final Logger LOGGER = LogUtils.getLogger();
    private static final Gson JSON = new Gson();
    private static final Path DIRECTORY = FMLPaths.CONFIGDIR.get().resolve("flyminer");
    private static final Path COMMAND = DIRECTORY.resolve("command.json");
    private static final Set<Block> STONE = Set.of(Blocks.STONE, Blocks.COBBLESTONE, Blocks.DEEPSLATE,
        Blocks.COBBLED_DEEPSLATE, Blocks.GRANITE, Blocks.DIORITE, Blocks.ANDESITE, Blocks.TUFF,
        Blocks.DIRT, Blocks.GRASS_BLOCK, Blocks.COARSE_DIRT, Blocks.ROOTED_DIRT, Blocks.PODZOL, Blocks.CLAY);
    // Verified in ScorchedGuns 0.5.5 worldgen/loot data: host rock, not rich_phosphorite ore or a machine.
    private static final Set<String> MOD_TERRAIN = Set.of("scguns:phosphorite");
    private static final SessionGuard SESSION = new SessionGuard();
    private static boolean enabled;
    private static int ticks;
    private static BlockPos target;
    private static Block originalBlock;
    private static Direction targetFace;
    private static KeyMapping walkingKey;
    private static String walkDirection;
    private static int remainingTicks;
    private static String activeId;
    private static String lastId = "";
    private static String lastStatus = "idle";
    private static String lastMessage = "Use /flyminer enable in the game to allow local commands.";
    private static boolean eating;
    private static int foodBefore;
    private static LivingEntity enemy;
    private static int changedTicks;
    private static BlockPos moveOrigin;
    private static BlockPos moveTarget;
    private static boolean jumpStarted;
    private static String miningTarget = "";
    private static BlockPos scanOrigin, scanCenter;
    private static int scanHeight, scanCursor;
    private static int[] scanCells;
    private static final ArrayList<Map<String, Object>> scanPalette = new ArrayList<>();
    private static final Map<Long, Integer> scanStates = new java.util.HashMap<>();
    private static long commandDeadline;
    private static BlockPos torchTarget;
    private static BlockPos centerTarget;
    private static boolean travelling;
    private static boolean returning;
    private static final ArrayList<Map<String, Integer>> scanChests = new ArrayList<>();
    private static final java.util.ArrayDeque<BlockPos> moveQueue = new java.util.ArrayDeque<>();
    private static boolean combatApproach;

    private ClientBridge() {}

    public static void initialize() {
        try { Files.createDirectories(DIRECTORY); HomeSettings.load(); MiningProfile.load(); }
        catch (IOException error) { LOGGER.error("Cannot create flyminer control directory", error); return; }
        MinecraftForge.EVENT_BUS.addListener(ClientBridge::registerCommands);
        MinecraftForge.EVENT_BUS.addListener(HomeSettings::register);
        MinecraftForge.EVENT_BUS.addListener(ClientBridge::tick);
        MinecraftForge.EVENT_BUS.addListener(ClientBridge::guardGoggles);
        LOGGER.info("Fly Miner Bridge ready. Local commands are disabled until /flyminer enable.");
    }

    private static void guardGoggles(RenderGuiOverlayEvent.Pre event) {
        // TFMG's injected goggle renderer casts EntityHitResult to BlockHitResult (observed log).
        // Suppress only this incompatible overlay; never change Minecraft's combat hit result.
        if (ModList.get().isLoaded("tfmg") && event.getOverlay().id().toString().equals("create:goggle_info") &&
            !(Minecraft.getInstance().hitResult instanceof BlockHitResult)) event.setCanceled(true);
    }

    private static void registerCommands(RegisterClientCommandsEvent event) {
        event.getDispatcher().register(Commands.literal("flyminer")
            .then(Commands.literal("target")
                .executes(context -> { localMessage("Target: " + (miningTarget.isEmpty() ? "any ore" : miningTarget)); return 1; })
                .then(Commands.literal("clear").executes(context -> {
                    stop(Minecraft.getInstance(), "stopped", "Mining target changed.");
                    miningTarget = "";
                    writeState(Minecraft.getInstance());
                    localMessage("Target cleared; normal ore scores restored.");
                    return 1;
                }))
                .then(Commands.argument("ore", ResourceLocationArgument.id())
                    .suggests((context, builder) -> SharedSuggestionProvider.suggestResource(
                        oreIds().stream().map(ResourceLocation::new), builder))
                    .executes(context -> {
                        ResourceLocation id = ResourceLocationArgument.getId(context, "ore");
                        if (id == null || !oreIds().contains(id.toString())) {
                            localMessage("Unknown mineable ore. Press Tab to see registered ore blocks."); return 0;
                        }
                        stop(Minecraft.getInstance(), "stopped", "Mining target changed.");
                        miningTarget = id.toString();
                        writeState(Minecraft.getInstance());
                        localMessage("Priority ore: " + miningTarget + " (100 points per block; matching vanilla deepslate variant included).");
                        return 1;
                    })))
            .then(Commands.literal("enable").executes(context -> {
                Minecraft mc = Minecraft.getInstance();
                if (!expectedSession(mc)) { localMessage("Open a singleplayer world or join a server first."); return 0; }
                stop(mc, "stopped", "Enabling local control for the current world.");
                SESSION.enable(controlSession(mc));
                enabled = true;
                localMessage("Local control enabled. /flyminer stop disables it. Opening a menu also stops actions.");
                return 1;
            }))
            .then(Commands.literal("stop").executes(context -> {
                enabled = false;
                SESSION.clear();
                stop(Minecraft.getInstance(), "stopped", "Stopped in game.");
                localMessage("Local control disabled.");
                return 1;
            }))
            .then(Commands.literal("status").executes(context -> {
                localMessage("Enabled=" + enabled + ", action=" + actionName() + ", " + lastMessage);
                return 1;
            })));
    }

    static void localMessage(String message) {
        Minecraft mc = Minecraft.getInstance();
        if (mc.player != null) mc.player.displayClientMessage(Component.literal("[Fly Miner] " + message), false);
    }

    private static java.util.List<String> oreIds() {
        Minecraft mc = Minecraft.getInstance();
        if (mc.level == null || mc.player == null) return java.util.List.of();
        return BuiltInRegistries.BLOCK.stream()
            .filter(block -> {
                try { return NetherSafety.ore(block.defaultBlockState()) && block.defaultBlockState().getDestroySpeed(mc.level,mc.player.blockPosition())>=0; }
                catch(RuntimeException error){return false;}
            })
            .map(block -> BuiltInRegistries.BLOCK.getKey(block).toString()).sorted().toList();
    }

    static boolean expectedSession(Minecraft mc) {
        return mc.player != null && mc.level != null && mc.gameMode != null &&
            (mc.hasSingleplayerServer() || mc.getCurrentServer() != null);
    }

    private static String controlSession(Minecraft mc) {
        return sessionId(mc) + "|" + mc.player.getGameProfile().getId() + "|" + mc.level.dimension().location();
    }

    static String sessionId(Minecraft mc) {
        var server = mc.getSingleplayerServer();
        if (server != null) {
            String path = server.getWorldPath(net.minecraft.world.level.storage.LevelResource.ROOT).toAbsolutePath().normalize().toString();
            return "singleplayer:" + server.getWorldData().getLevelName() + ":" + Integer.toHexString(path.hashCode());
        }
        return mc.getCurrentServer() == null ? "disconnected" : mc.getCurrentServer().ip;
    }

    private static void tick(TickEvent.ClientTickEvent event) {
        if (event.phase != TickEvent.Phase.END) return;
        Minecraft mc = Minecraft.getInstance();
        if (!expectedSession(mc) || mc.player.isDeadOrDying() || enabled && !SESSION.allows(controlSession(mc))) {
            enabled = false;
            SESSION.clear();
            if (activeId != null) stop(mc, "stopped", "Session ended or player died.");
        } else if (mc.screen != null && !ChestTransfer.owned(mc) && !PickaxeCraft.owned(mc) || mc.isPaused()) {
            if (activeId != null) stop(mc, "stopped", "A game menu opened.");
        }
        readCommand(mc);
        if (activeId != null) {
            try { updateAction(mc); }
            catch (Exception error) { LOGGER.warn("Flyminer action failed: {}", actionName(), error); stop(mc, "error", error.getMessage()); }
        }
        if (++ticks % 2 == 0) writeState(mc);
    }

    private static void readCommand(Minecraft mc) {
        if (!Files.isRegularFile(COMMAND)) return;
        String id = "invalid";
        try {
            if (Files.size(COMMAND) > 4096) { Files.delete(COMMAND); throw new IllegalArgumentException("Command is too large."); }
            String text = Files.readString(COMMAND, StandardCharsets.UTF_8);
            Files.delete(COMMAND);
            JsonObject command = JSON.fromJson(text, JsonObject.class);
            id = command.get("id").getAsString();
            if (!id.matches("[a-zA-Z0-9-]{1,64}")) throw new IllegalArgumentException("Invalid command ID.");
            long age = System.currentTimeMillis() - command.get("createdAt").getAsLong();
            if (age < -1000 || age > 10000) throw new IllegalArgumentException("Command expired.");
            String action = command.get("action").getAsString();
            if (action.equals("stop")) {
                stop(mc, "stopped", "Stopped from local controller.");
                result(id, "done", "All actions stopped.");
                return;
            }
            if (!enabled || !expectedSession(mc)) throw new IllegalStateException("Enable with /flyminer enable in the game first.");
            if (mc.screen != null || mc.isPaused()) throw new IllegalStateException("Close the game menu first.");
            if (activeId != null) throw new IllegalStateException("Another action is running. Stop it first.");
            commandDeadline = System.currentTimeMillis() + 5000;
            travelling = command.has("travel") && command.get("travel").getAsBoolean();
            returning = travelling && command.has("returning") && command.get("returning").getAsBoolean();
            if (!action.equals("breathe") && AirSupply.urgent(mc))
                throw new IllegalStateException("Oxygen recovery preempts work; return to breathable air.");
            if (Set.of("mine","traverse","route","step","center","torch","store","craft_pickaxe","open_passage","approach").contains(action))
                MiningProfile.requireCover(mc);
            if (Set.of("mine","traverse","route","center","torch","store","craft_pickaxe","open_passage").contains(action))
                NetherSafety.requireWorkSafe(mc);
            switch (action) {
                case "breathe" -> {
                    BreathEscape.begin(mc); activeId=id; remainingTicks=500;
                    result(id,"running","Finding breathable air; checking exit terrain and emergency tools.");
                }
                case "home_request" -> {
                    var p = HomeSettings.get(mc);
                    if (p.home == null) throw new IllegalStateException("Set home first.");
                    p.request = Math.max(p.request + 1, System.currentTimeMillis());
                    HomeSettings.save();
                    result(id, "done", "Automatic return request saved.");
                }
                case "home_ack" -> {
                    HomeSettings.acknowledge(mc, command.get("request").getAsLong());
                    result(id, "done", "Home request acknowledged.");
                }
                case "store" -> {
                    BlockPos chest = new BlockPos(coordinate(command, "x"), coordinate(command, "y"), coordinate(command, "z"));
                    ChestTransfer.begin(mc, chest);
                    activeId = id; remainingTicks = 1200;
                    result(id, "running", "Opening home chest and depositing/refilling supplies.");
                }
                case "craft_pickaxe" -> {
                    PickaxeCraft.begin(mc); activeId=id; remainingTicks=3500;
                    result(id,"running","Crafting best available pickaxe; preparing workstations/smelting if needed.");
                }
                case "open_passage" -> {
                    Passages.begin(mc,new BlockPos(coordinate(command,"x"),coordinate(command,"y"),coordinate(command,"z")));
                    activeId=id;remainingTicks=70;
                    result(id,"running","Opening a door or fence gate without breaking it.");
                }
                case "scan" -> beginScan(mc, id);
                case "center" -> beginCenter(mc, id);
                case "torch" -> beginTorch(mc, command, id);
                case "escape" -> {
                    if (!RecoveryMove.begin(mc, command.has("avoidThreats") && command.get("avoidThreats").getAsBoolean())) { result(id,"skipped","No supported dry escape corridor available yet."); break; }
                    activeId=id; remainingTicks=80;
                    result(id,"running","Retreating along a checked supported corridor.");
                }
                case "look" -> {
                    float yaw = command.get("yaw").getAsFloat();
                    float pitch = command.get("pitch").getAsFloat();
                    if (!Float.isFinite(yaw) || !Float.isFinite(pitch) || Math.abs(yaw) > 360 || Math.abs(pitch) > 90)
                        throw new IllegalArgumentException("Invalid camera angle.");
                    mc.player.setYRot(yaw);
                    mc.player.setXRot(pitch);
                    result(id, "done", "Camera rotated.");
                }
                case "mine" -> beginMine(mc, command, id);
                case "step" -> beginStep(mc, command, id);
                case "traverse" -> beginTraverse(mc, command, id);
                case "route" -> beginRoute(mc, command, id);
                case "approach" -> {
                    var entity=mc.level.getEntity(command.get("entityId").getAsInt());
                    if(!(entity instanceof LivingEntity living)||!hostileToPlayer(mc,living)||!living.getUUID().toString().equals(command.get("uuid").getAsString())||!mc.player.hasLineOfSight(living))
                        throw new IllegalStateException("Approach target changed.");
                    BlockPos to=new BlockPos(coordinate(command,"x"),coordinate(command,"y"),coordinate(command,"z"));
                    if(to.getY()!=mc.player.blockPosition().getY()||Vec3.atBottomCenterOf(to).distanceTo(living.position())>=mc.player.distanceTo(living)||living instanceof net.minecraft.world.entity.monster.Creeper c && c.getSwellDir()>0)
                        throw new IllegalStateException("Unsafe combat approach.");
                    equip(mc,"sword");combatApproach=true;beginTraverse(mc,command,id);
                }
                case "equip" -> {
                    equip(mc, command.get("kind").getAsString());
                    result(id, "done", "Equipment selected.");
                }
                case "eat" -> {
                    if (!mc.player.getFoodData().needsFood()) { result(id, "done", "Not hungry."); break; }
                    equip(mc, "food");
                    eating = true;
                    foodBefore = mc.player.getFoodData().getFoodLevel();
                    activeId = id;
                    remainingTicks = 100;
                    mc.gameMode.useItem(mc.player, InteractionHand.MAIN_HAND);
                    mc.options.keyUse.setDown(true);
                    result(id, "running", "Eating one food item.");
                }
                case "attack" -> {
                    var entity = mc.level.getEntity(command.get("entityId").getAsInt());
                    if (!(entity instanceof LivingEntity living) || !hostileToPlayer(mc, living) ||
                        !living.getUUID().toString().equals(command.get("uuid").getAsString()))
                        throw new IllegalStateException("Enemy changed or is not hostile.");
                    validateAttack(mc, living);
                    equip(mc, "sword");
                    enemy = living;
                    activeId = id;
                    remainingTicks = 40;
                    result(id, "running", "Waiting for sword cooldown.");
                }
                default -> throw new IllegalArgumentException("Unknown action.");
            }
        } catch (Exception error) {
            if (activeId != null || PickaxeCraft.active() || ChestTransfer.active() || Passages.active() || BreathEscape.active()) stop(mc,"error",error.getMessage());
            combatApproach=false;
            result(id, "error", error.getMessage());
        }
        writeState(mc);
    }

    private static int coordinate(JsonObject command, String key) {
        double value = command.get(key).getAsDouble();
        if (!Double.isFinite(value) || value != Math.rint(value) || Math.abs(value) > 30000000)
            throw new IllegalArgumentException("Invalid block coordinate.");
        return (int) value;
    }

    private static void beginMine(Minecraft mc, JsonObject command, String id) {
        BlockPos position = new BlockPos(coordinate(command, "x"), coordinate(command, "y"), coordinate(command, "z"));
        ToolChoice.equip(mc, mc.level.getBlockState(position));
        BlockHitResult hit = validateMine(mc, position);
        BlockState state = mc.level.getBlockState(position);
        Vec3 delta = hit.getLocation().subtract(mc.player.getEyePosition());
        mc.player.setYRot((float) (Math.toDegrees(Math.atan2(delta.z, delta.x)) - 90));
        mc.player.setXRot((float) -Math.toDegrees(Math.atan2(delta.y, Math.hypot(delta.x, delta.z))));
        if (!mc.gameMode.startDestroyBlock(position, hit.getDirection())) throw new IllegalStateException("The game refused to start mining.");
        target = position;
        originalBlock = state.getBlock();
        targetFace = hit.getDirection();
        activeId = id;
        remainingTicks = 400;
        changedTicks = 0;
        result(id, "running", "Mining one " + BuiltInRegistries.BLOCK.getKey(originalBlock));
    }

    private static BlockHitResult validateMine(Minecraft mc, BlockPos position) {
        return validateMine(mc, position, true);
    }
    private static BlockHitResult validateMine(Minecraft mc, BlockPos position, boolean equippedTool) {
        MiningProfile.requireCover(mc);
        if (!MiningProfile.mayMine(mc,position)) throw new IllegalStateException("Dwarf profile: this cut would open the roof to the sky.");
        NetherSafety.requireWorkSafe(mc);
        LiquidSafety.requirePlayerClear(mc);
        if (HomeSettings.protectedAt(mc, position)) throw new IllegalStateException("Home protected area: digging is forbidden at every height.");
        if (supportsTorch(mc, position)) throw new IllegalStateException("This block supports a torch; choose another cut.");
        if (mc.player.isInWater() || mc.player.isInLava() || !mc.player.onGround())
            throw new IllegalStateException("Mining requires dry, supported ground.");
        if (mc.player.getHealth() < 18) throw new IllegalStateException("Low health; pause mining.");
        if (!mc.level.getEntitiesOfClass(LivingEntity.class, mc.player.getBoundingBox().inflate(3.5),
            entity -> hostileToPlayer(mc, entity) && mc.player.hasLineOfSight(entity)).isEmpty())
            throw new IllegalStateException("Enemy nearby; interrupt mining.");
        if (!mc.level.hasChunkAt(position)) throw new IllegalStateException("Target chunk is not loaded.");
        BlockState state = mc.level.getBlockState(position);
        if (state.hasBlockEntity() || (!isTerrain(state) && !NetherSafety.ore(state)))
            throw new IllegalArgumentException("Only approved natural terrain and ore blocks may be mined.");
        if (!NetherSafety.miningAllowed(mc,position,state))
            throw new IllegalStateException("Piglin near guarded gold; do not provoke it.");
        if (NetherSafety.active(mc) && position.getX()==mc.player.blockPosition().getX()
            && position.getZ()==mc.player.blockPosition().getZ() && position.getY()>=StandingGeometry.feet(mc).getY()+2
            && !NetherSafety.coveredAfterCut(mc,position))
            throw new IllegalStateException("Nether mining requires a stable roof; do not open the last cover block.");
        BlockPos feet = StandingGeometry.feet(mc);
        if (position.getX() == feet.getX() && position.getZ() == feet.getZ() && position.getY() < feet.getY())
            throw new IllegalStateException("Cannot mine beneath the player.");
        if (mc.player.getInventory().getFreeSlot() < 0 && !returning) throw new IllegalStateException("Inventory is full.");
        if (state.requiresCorrectToolForDrops() && (equippedTool
            ? !mc.player.hasCorrectToolForDrops(state) : !ToolChoice.harvestable(mc, state)))
            throw new IllegalStateException("Hold a suitable pickaxe first.");
        if (state.getDestroySpeed(mc.level, position) < 0) throw new IllegalStateException("Unbreakable block.");
        if (!LiquidSafety.cellClear(mc,position)) throw new IllegalStateException("Water/lava or unknown terrain within two blocks of the mining target.");
        if (mc.level.getBlockState(position.above()).getBlock() instanceof net.minecraft.world.level.block.FallingBlock)
            throw new IllegalStateException("Falling block above target.");
        Vec3 eye = mc.player.getEyePosition();
        Vec3 center = Vec3.atCenterOf(position);
        if (eye.distanceTo(center) > mc.gameMode.getPickRange()) throw new IllegalStateException("Target is out of reach.");
        BlockHitResult hit = mc.level.clip(new ClipContext(eye, center, ClipContext.Block.OUTLINE, ClipContext.Fluid.NONE, mc.player));
        if (hit.getType() != HitResult.Type.BLOCK || !hit.getBlockPos().equals(position))
            throw new IllegalStateException("Target is hidden behind another block.");
        return hit;
    }

    private static void beginStep(Minecraft mc, JsonObject command, String id) {
        String direction = command.get("direction").getAsString();
        double duration = command.get("ticks").getAsDouble();
        if (!Double.isFinite(duration) || duration != Math.rint(duration) || duration < 1 || duration > 20)
            throw new IllegalArgumentException("Step duration must be 1 to 20 game ticks.");
        KeyMapping key = switch (direction) {
            case "forward" -> mc.options.keyUp;
            case "back" -> mc.options.keyDown;
            case "left" -> mc.options.keyLeft;
            case "right" -> mc.options.keyRight;
            default -> throw new IllegalArgumentException("Invalid step direction.");
        };
        validateStep(mc, direction);
        walkingKey = key;
        walkDirection = direction;
        remainingTicks = (int) duration;
        activeId = id;
        key.setDown(true);
        result(id, "running", "Walking " + direction);
    }

    private static void validateStep(Minecraft mc, String direction) {
        LiquidSafety.requirePlayerClear(mc);
        if (!mc.player.onGround() || mc.player.isInWater() || mc.player.isPassenger())
            throw new IllegalStateException("Steps require standing on dry ground.");
        double angle = Math.toRadians(mc.player.getYRot() + switch (direction) {
            case "back" -> 180; case "left" -> -90; case "right" -> 90; default -> 0;
        });
        BlockPos next = BlockPos.containing(mc.player.position().add(-Math.sin(angle) * 0.8, 0, Math.cos(angle) * 0.8));
        if (!safeFloor(mc, next) || !clearAt(mc, next) || !clearAt(mc, next.above()))
            throw new IllegalStateException("Step would approach a ledge or hazard.");
    }

    private static boolean safeFloor(Minecraft mc, BlockPos feet) {
        try { return checkedFloor(mc,feet); } catch(RuntimeException error){return false;}
    }
    private static boolean checkedFloor(Minecraft mc, BlockPos feet) {
        if (!mc.level.hasChunkAt(feet)) return false;
        if (MiningProfile.exposed(mc,feet) || MiningProfile.exposed(mc,feet.above())) return false;
        if (!LiquidSafety.bodyClear(mc,feet)) return false;
        BlockState floor = mc.level.getBlockState(feet.below());
        return Passages.supports(mc,feet.below(),floor) && !dangerous(floor) &&
            !(floor.getBlock() instanceof net.minecraft.world.level.block.FallingBlock) &&
            !dangerous(mc.level.getBlockState(feet)) && !dangerous(mc.level.getBlockState(feet.above()));
    }

    // A player can stand with their center over air while their feet overlap an adjacent block.
    // Recenter only along a collision-free, continuously supported horizontal sweep of the real body.
    private static boolean supportedBody(Minecraft mc, net.minecraft.world.phys.AABB box) {
        if (!LiquidSafety.bodyClear(mc,box)) return false;
        return supportedDryBody(mc,box);
    }
    static boolean supportedDryBody(Minecraft mc, net.minecraft.world.phys.AABB box) {
        if (!MiningProfile.safeBody(mc,box)) return false;
        if (!mc.level.noCollision(mc.player, box.deflate(.00001))) return false;
        var footing = new net.minecraft.world.phys.AABB(box.minX + .00001, box.minY - .06, box.minZ + .00001,
            box.maxX - .00001, box.minY, box.maxZ - .00001);
        for (BlockPos p : BlockPos.betweenClosed(BlockPos.containing(footing.minX, footing.minY, footing.minZ),
                BlockPos.containing(box.maxX - .00001, box.maxY - .00001, box.maxZ - .00001))) {
            if (!mc.level.hasChunkAt(p) || dangerous(mc.level.getBlockState(p)) ||
                p.getY()<box.minY && mc.level.getBlockState(p).getBlock() instanceof net.minecraft.world.level.block.FallingBlock) return false;
        }
        for (var shape : mc.level.getBlockCollisions(mc.player, footing)) if (!shape.isEmpty()) return true;
        return false;
    }

    private static boolean safeCenterPath(Minecraft mc, BlockPos destination) {
        if (!mc.player.onGround() || mc.player.isInWater() || mc.player.isInLava() || mc.player.isPassenger() ||
            !StandingGeometry.atHeight(mc.player.getY(), StandingGeometry.floorY(mc, destination)) || !safeFloor(mc, destination) ||
            !clearAt(mc, destination) || !clearAt(mc, destination.above())) return false;
        double dx = destination.getX() + .5 - mc.player.getX();
        double dz = destination.getZ() + .5 - mc.player.getZ();
        if (Math.hypot(dx, dz) > 1.5) return false;
        int slices = Math.max(1, (int) Math.ceil(Math.hypot(dx, dz) / .08));
        for (int i = 0; i <= slices; i++) {
            double fraction = (double) i / slices;
            if (!supportedBody(mc, mc.player.getBoundingBox().move(dx * fraction, 0, dz * fraction))) return false;
        }
        return true;
    }

    private static BlockPos findAnchor(Minecraft mc) {
        BlockPos feet = StandingGeometry.feet(mc);
        var candidates = new ArrayList<BlockPos>();
        for (int x = -1; x <= 1; x++) for (int z = -1; z <= 1; z++) candidates.add(feet.offset(x, 0, z));
        candidates.sort(java.util.Comparator.comparingDouble(p ->
            mc.player.position().distanceToSqr(new Vec3(p.getX() + .5, p.getY(), p.getZ() + .5))));
        for (BlockPos p : candidates) if (safeCenterPath(mc, p)) return p;
        return null;
    }

    private static void validateCenter(Minecraft mc, BlockPos destination) {
        if (mc.player.getHealth() < (travelling ? 9 : 18) || !safeCenterPath(mc, destination))
            throw new IllegalStateException("No continuously supported path to the standing anchor.");
        if (!mc.level.getEntitiesOfClass(LivingEntity.class, mc.player.getBoundingBox().inflate(3.5),
            entity -> hostileToPlayer(mc, entity) && mc.player.hasLineOfSight(entity)).isEmpty())
            throw new IllegalStateException("Enemy nearby; interrupt centering.");
    }

    private static void beginCenter(Minecraft mc, String id) {
        BlockPos anchor = findAnchor(mc);
        if (anchor == null) throw new IllegalStateException("No nearby standing anchor with continuous floor support.");
        validateCenter(mc, anchor);
        centerTarget = anchor; walkingKey = mc.options.keyUp;
        activeId = id; remainingTicks = 60;
        result(id, "running", "Centering on a checked, supported floor.");
    }

    private static void updateCenter(Minecraft mc) {
        validateCenter(mc, centerTarget);
        double dx = centerTarget.getX() + .5 - mc.player.getX();
        double dz = centerTarget.getZ() + .5 - mc.player.getZ();
        double distance = Math.hypot(dx, dz);
        double speed = mc.player.getDeltaMovement().horizontalDistance();
        if (distance < .12 && speed < .025) { stop(mc, "done", "Centered on supported ground."); return; }
        mc.player.setYRot((float) Math.toDegrees(Math.atan2(-dx, dz)));
        mc.player.setXRot(0);
        mc.player.setSprinting(false);
        walkingKey.setDown(distance > Math.max(.07, speed * 2.2 + .04));
    }

    // Each route is one cardinal cell and at most one level. Clear a staircase,
    // never the block supporting the player or the destination's landing floor.
    private static java.util.List<BlockPos> clearance(BlockPos from, BlockPos to) {
        int rise = to.getY() - from.getY();
        if (rise > 0) return java.util.List.of(from.above(2), to.above(), to);
        if (rise < 0) return java.util.List.of(to.above(2), to.above(), to);
        return java.util.List.of(to.above(), to);
    }

    private static boolean clearAt(Minecraft mc, BlockPos pos) {
        try { return checkedClear(mc,pos); } catch(RuntimeException error){return false;}
    }
    private static boolean checkedClear(Minecraft mc, BlockPos pos) {
        if (!mc.level.hasChunkAt(pos)) return false;
        if (MiningProfile.exposed(mc,pos)) return false;
        BlockState block = mc.level.getBlockState(pos);
        return !dangerous(block) && Passages.clear(mc,pos,block);
    }

    private static void validateTraverse(Minecraft mc, BlockPos from, BlockPos to) {
        MiningProfile.requireCover(mc);
        AirSupply.requireEntry(mc,to);
        NetherSafety.requireWorkSafe(mc);
        LiquidSafety.requirePlayerClear(mc);
        if (to.getY()>from.getY() && (!LiquidSafety.cellClear(mc,from.above(3)) || !LiquidSafety.cellClear(mc,to.above(2))))
            throw new IllegalStateException("Water/lava or unknown terrain within two blocks of the jump headroom.");
        if (mc.player.isInWater() || mc.player.isInLava() || mc.player.isPassenger() || mc.player.getHealth() < (travelling || combatApproach ? 9 : 18))
            throw new IllegalStateException("Traversal interrupted: health or movement state changed.");
        if (!safeFloor(mc, from)) throw new IllegalStateException("Traversal origin has no safe center support; recenter first.");
        if (!safeFloor(mc, to)) throw new IllegalStateException("Traversal destination floor changed or has a hazard.");
        if (clearance(from, to).stream().anyMatch(pos -> !clearAt(mc, pos)))
            throw new IllegalStateException("Traversal clearance changed or is blocked.");
        if (!combatApproach && !mc.level.getEntitiesOfClass(LivingEntity.class, mc.player.getBoundingBox().inflate(3.5),
            entity -> hostileToPlayer(mc, entity) && mc.player.hasLineOfSight(entity)).isEmpty())
            throw new IllegalStateException("Enemy nearby; interrupt traversal.");
        // Reject lateral drift, teleports and a fall outside the planned one-block step.
        Vec3 p = mc.player.position();
        double lateral = from.getX() == to.getX() ? Math.abs(p.x - (from.getX() + .5)) : Math.abs(p.z - (from.getZ() + .5));
        if (lateral > .3 || p.x < Math.min(from.getX(), to.getX()) + .1 || p.x > Math.max(from.getX(), to.getX()) + .9 ||
            p.z < Math.min(from.getZ(), to.getZ()) + .1 || p.z > Math.max(from.getZ(), to.getZ()) + .9 ||
            p.y < Math.min(from.getY(), to.getY()) - .15 || p.y > Math.max(from.getY(), to.getY()) + 1.5)
            throw new IllegalStateException("Traversal left its checked corridor at " + p + "; from " + from + " to " + to);
    }

    private static void beginTraverse(Minecraft mc, JsonObject command, String id) {
        moveQueue.clear();
        BlockPos from = StandingGeometry.feet(mc);
        BlockPos to = new BlockPos(coordinate(command, "x"), coordinate(command, "y"), coordinate(command, "z"));
        if (!mc.player.onGround() || Math.abs(to.getX() - from.getX()) + Math.abs(to.getZ() - from.getZ()) != 1 ||
            Math.abs(to.getY() - from.getY()) > 1)
            throw new IllegalArgumentException("Traversal requires an adjacent cell, at most one level up or down.");
        validateTraverse(mc, from, to);
        moveOrigin = from;
        moveTarget = to;
        jumpStarted = false;
        walkingKey = mc.options.keyUp;
        activeId = id;
        remainingTicks = 60;
        result(id, "running", "Traversing checked staircase/level route.");
    }

    private static void beginRoute(Minecraft mc, JsonObject command, String id) {
        var path = command.getAsJsonArray("path");
        if (path == null || path.size() < 2 || path.size() > 8) throw new IllegalArgumentException("Route needs 2..8 checked cells.");
        BlockPos from = StandingGeometry.feet(mc);
        var checked = new java.util.ArrayDeque<BlockPos>();
        int dx = 0, dz = 0;
        for (var entry : path) {
            var p = entry.getAsJsonObject();
            BlockPos to = new BlockPos(coordinate(p,"x"),coordinate(p,"y"),coordinate(p,"z"));
            if (checked.isEmpty()) { dx=to.getX()-from.getX(); dz=to.getZ()-from.getZ(); }
            if (Math.abs(dx)+Math.abs(dz)!=1 || to.getY()!=from.getY() || to.getX()-from.getX()!=dx || to.getZ()-from.getZ()!=dz ||
                !safeFloor(mc,to) || clearance(from,to).stream().anyMatch(q -> !clearAt(mc,q)))
                throw new IllegalStateException("Route contains a corner, unknown floor or obstruction.");
            checked.add(to); from=to;
        }
        beginTraverse(mc,path.get(0).getAsJsonObject(),id);
        checked.removeFirst(); moveQueue.addAll(checked); remainingTicks=200;
    }

    private static void updateTraverse(Minecraft mc) {
        validateTraverse(mc, moveOrigin, moveTarget);
        double dx = moveTarget.getX() + .5 - mc.player.getX();
        double dz = moveTarget.getZ() + .5 - mc.player.getZ();
        double distance = Math.hypot(dx, dz);
        double speed = mc.player.getDeltaMovement().horizontalDistance();
        if (!moveQueue.isEmpty() && distance < .30 && mc.player.onGround()) {
            moveOrigin=moveTarget; moveTarget=moveQueue.removeFirst();
            validateTraverse(mc,moveOrigin,moveTarget);
            dx=moveTarget.getX()+.5-mc.player.getX(); dz=moveTarget.getZ()+.5-mc.player.getZ(); distance=Math.hypot(dx,dz);
        }
        if (distance < .14 && speed < .025) {
            walkingKey.setDown(false);
            mc.options.keyJump.setDown(false);
            if (StandingGeometry.landed(mc, moveTarget))
                stop(mc, "done", "Reached checked route destination.");
            return;
        }
        mc.player.setYRot((float) Math.toDegrees(Math.atan2(-dx, dz)));
        mc.player.setXRot(0);
        boolean up = moveTarget.getY() > moveOrigin.getY();
        // One jump only. Release in flight to avoid bouncing again after landing.
        if (up && !jumpStarted && mc.player.onGround()) {
            mc.options.keyJump.setDown(true);
            jumpStarted = true;
        } else mc.options.keyJump.setDown(false);
        mc.player.setSprinting(travelling && !moveQueue.isEmpty() && mc.player.getFoodData().getFoodLevel()>6);
        walkingKey.setDown(distance > Math.max(.07, speed * 2.2 + .04));
    }

    private static boolean dangerous(BlockState state) {
        return !state.getFluidState().isEmpty() || state.is(BlockTags.FIRE) || state.is(Blocks.MAGMA_BLOCK) ||
            state.is(Blocks.CAMPFIRE) || state.is(Blocks.SOUL_CAMPFIRE) || state.is(Blocks.CACTUS) ||
            state.is(Blocks.SWEET_BERRY_BUSH) || state.is(Blocks.POWDER_SNOW) || state.is(Blocks.WITHER_ROSE);
    }

    private static boolean isTerrain(BlockState state) {
        return STONE.contains(state.getBlock()) || NetherSafety.terrain(state) || MOD_TERRAIN.contains(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString());
    }

    static boolean matches(Minecraft mc, ItemStack stack, String kind) {
        try {return matchesChecked(mc,stack,kind);}catch(RuntimeException error){return false;}
    }
    private static boolean matchesChecked(Minecraft mc,ItemStack stack,String kind) {
        if (stack.isEmpty()) return false;
        if (stack.isDamageableItem() && stack.getMaxDamage() - stack.getDamageValue() <= 5) return false;
        return switch (kind) {
            case "pickaxe" -> stack.is(ItemTags.PICKAXES) || stack.canPerformAction(net.minecraftforge.common.ToolActions.PICKAXE_DIG);
            case "sword" -> stack.is(ItemTags.SWORDS) || stack.getItem() instanceof net.minecraft.world.item.SwordItem || stack.canPerformAction(net.minecraftforge.common.ToolActions.SWORD_DIG);
            case "torch" -> stack.is(net.minecraft.world.item.Items.TORCH);
            case "food" -> {
                var food = stack.getFoodProperties(mc.player);
                yield food != null && food.getNutrition() > 0 &&
                    food.getEffects().stream().allMatch(effect -> effect.getFirst().getEffect().isBeneficial());
            }
            default -> false;
        };
    }

    private static void equip(Minecraft mc, String kind) {
        if (!Set.of("pickaxe", "sword", "food", "torch").contains(kind)) throw new IllegalArgumentException("Unknown equipment kind.");
        var inventory = mc.player.getInventory();
        int selected = -1;
        double best = -1;
        // Prefer a tool that harvests the chosen ore even when another tool is faster.
        boolean targetToolAvailable = kind.equals("pickaxe") && inventory.items.stream().anyMatch(stack -> harvestsTarget(mc,stack));
        for (int slot = 0; slot < inventory.items.size(); slot++) {
            ItemStack stack = inventory.items.get(slot);
            if (!matches(mc, stack, kind)) continue;
            if (targetToolAvailable && !harvestsTarget(mc,stack)) continue;
            double score = kind.equals("pickaxe") ? stack.getDestroySpeed(Blocks.STONE.defaultBlockState()) +
                (stack.isCorrectToolForDrops(Blocks.DIAMOND_ORE.defaultBlockState()) ? 100 :
                    stack.isCorrectToolForDrops(Blocks.IRON_ORE.defaultBlockState()) ? 50 : 0) :
                kind.equals("food") ? stack.getFoodProperties(mc.player).getNutrition() :
                stack.getItem() instanceof net.minecraft.world.item.SwordItem sword ? sword.getDamage() : 1;
            if (score > best) { selected = slot; best = score; }
        }
        if (selected < 0) throw new IllegalStateException("Missing usable " + kind + ". Refill inventory to continue.");
        if (selected >= 9) {
            if (mc.player.containerMenu != mc.player.inventoryMenu) throw new IllegalStateException("Close inventory first.");
            mc.gameMode.handleInventoryMouseClick(mc.player.inventoryMenu.containerId, selected, 8, ClickType.SWAP, mc.player);
            selected = 8;
        }
        inventory.selected = selected;
    }

    static boolean hostileToPlayer(Minecraft mc, LivingEntity entity) {
        if (entity == mc.player || !entity.isAlive() || entity.isAlliedTo(mc.player)) return false;
        if (NetherSafety.active(mc) && NetherSafety.avoidOnly(entity)) return false;
        if (entity instanceof net.minecraft.world.entity.NeutralMob neutral && !neutral.isAngryAt(mc.player)) return false;
        return entity.isAlive() && (entity instanceof Enemy || entity.getType().getCategory()==net.minecraft.world.entity.MobCategory.MONSTER || entity instanceof Mob mob && mob.getTarget() == mc.player);
    }

    private static boolean canHarvestTarget(Minecraft mc) {
        return mc.player.getInventory().items.stream().anyMatch(stack -> harvestsTarget(mc, stack));
    }

    static boolean harvestsTarget(Minecraft mc, ItemStack stack) {
        if (!matches(mc, stack, "pickaxe")) return false;
        if (miningTarget.isEmpty()) return true;
        ResourceLocation id = ResourceLocation.tryParse(miningTarget);
        if (id == null) return false;
        BlockState block = BuiltInRegistries.BLOCK.get(id).defaultBlockState();
        return !block.requiresCorrectToolForDrops() || stack.isCorrectToolForDrops(block);
    }

    private static boolean supportsTorch(Minecraft mc, BlockPos support) {
        if (mc.level.getBlockState(support.above()).is(Blocks.TORCH)) return true;
        for (Direction face:Direction.Plane.HORIZONTAL) {
            BlockState neighbor=mc.level.getBlockState(support.relative(face));
            if (neighbor.is(Blocks.WALL_TORCH) && neighbor.getValue(net.minecraft.world.level.block.WallTorchBlock.FACING)==face) return true;
        }
        return false;
    }
    private static void beginTorch(Minecraft mc, JsonObject command, String id) {
        NetherSafety.requireWorkSafe(mc);
        LiquidSafety.requirePlayerClear(mc);
        if (!mc.player.onGround() || mc.player.isInWater() || mc.player.isInLava())
            throw new IllegalStateException("Torch placement needs dry ground.");
        BlockPos feet = StandingGeometry.feet(mc);
        if (!command.has("forwardX") || !command.has("forwardZ")) {
            result(id,"skipped","No planned digging direction supplied; leaving torches in inventory."); return;
        }
        int dx=coordinate(command,"forwardX"), dz=coordinate(command,"forwardZ");
        var avoid=new java.util.HashSet<BlockPos>();
        if (command.has("avoid")) {
            var entries=command.getAsJsonArray("avoid");
            if(entries.size()>128)throw new IllegalArgumentException("Too many planned cuts.");
            for(var entry:entries){var p=entry.getAsJsonObject();avoid.add(new BlockPos(coordinate(p,"x"),coordinate(p,"y"),coordinate(p,"z")));}
        }
        for (var spot:TorchPlacement.candidates(feet,dx,dz,avoid)) {
            BlockPos pos=spot.position(), support=spot.support();
            Direction face=spot.face();
            BlockState torch=face==Direction.UP ? Blocks.TORCH.defaultBlockState() :
                Blocks.WALL_TORCH.defaultBlockState().setValue(net.minecraft.world.level.block.WallTorchBlock.FACING,face);
            if (!mc.level.hasChunkAt(pos) || !mc.level.hasChunkAt(support) ||
                !mc.level.getBlockState(pos).isAir() || mc.level.getBlockState(support).hasBlockEntity() ||
                dangerous(mc.level.getBlockState(support)) || !LiquidSafety.cellClear(mc,pos) ||
                !torch.canSurvive(mc.level,pos)) continue;
            Vec3 point=Vec3.atCenterOf(support).add(Vec3.atLowerCornerOf(face.getNormal()).scale(.499));
            if (mc.player.getEyePosition().distanceTo(point) > mc.gameMode.getPickRange()) continue;
            BlockHitResult hit = mc.level.clip(new ClipContext(mc.player.getEyePosition(), point,
                ClipContext.Block.OUTLINE, ClipContext.Fluid.NONE, mc.player));
            if (hit.getType() != HitResult.Type.BLOCK || !hit.getBlockPos().equals(support) || hit.getDirection() != face) continue;
            equip(mc, "torch");
            Vec3 delta = point.subtract(mc.player.getEyePosition());
            mc.player.setYRot((float) (Math.toDegrees(Math.atan2(delta.z, delta.x)) - 90));
            mc.player.setXRot((float) -Math.toDegrees(Math.atan2(delta.y, Math.hypot(delta.x, delta.z))));
            var placed = mc.gameMode.useItemOn(mc.player, InteractionHand.MAIN_HAND, hit);
            if (placed.consumesAction()) {
                mc.player.swing(InteractionHand.MAIN_HAND); torchTarget = pos;
                activeId = id; remainingTicks = 40; changedTicks = 0;
                result(id, "running", "Placing a torch behind the planned cuts (wall preferred)."); return;
            }
        }
        result(id,"skipped","No rear torch support outside the planned cuts; continuing without placement.");
    }

    private static void validateAttack(Minecraft mc, LivingEntity entity) {
        if (!hostileToPlayer(mc, entity) || attackDistance(mc,entity) > 3.0 || !mc.player.hasLineOfSight(entity))
            throw new IllegalStateException("Enemy out of sword reach or hidden.");
    }
    private static double attackDistance(Minecraft mc, LivingEntity entity) {
        var eye=mc.player.getEyePosition(); var b=entity.getBoundingBox();
        return eye.distanceTo(new Vec3(net.minecraft.util.Mth.clamp(eye.x,b.minX,b.maxX),
            net.minecraft.util.Mth.clamp(eye.y,b.minY,b.maxY),net.minecraft.util.Mth.clamp(eye.z,b.minZ,b.maxZ)));
    }

    private static Map<String, Object> describeBlock(Minecraft mc, BlockPos position) {
        Map<String, Object> info = properties(mc, position);
        info.put("x", position.getX()); info.put("y", position.getY()); info.put("z", position.getZ());
        try { validateMine(mc, position, false); info.put("mineable", true); }
        catch (RuntimeException error) { info.put("mineable", false); info.put("reason", error.getMessage()); }
        return info;
    }

    private static Map<String, Object> properties(Minecraft mc, BlockPos pos) {
        try { return blockProperties(mc,pos); }
        catch (RuntimeException error) {
            if (ticks % 200 == 0) LOGGER.warn("Cannot inspect block at {}; treating it as unknown",pos,error);
            return new LinkedHashMap<>(Map.of("block","unknown","danger",true,"seconds",3600));
        }
    }
    private static Map<String, Object> blockProperties(Minecraft mc, BlockPos pos) {
        Map<String, Object> info = new LinkedHashMap<>();
        if (!mc.level.hasChunkAt(pos) || mc.level.isOutsideBuildHeight(pos)) {
            info.put("block", "unknown"); return info;
        }
        BlockState block = mc.level.getBlockState(pos);
        float hardness = block.getDestroySpeed(mc.level, pos);
        boolean correct = ToolChoice.harvestable(mc, block);
        boolean exposed = MiningProfile.exposed(mc,pos);
        info.put("block", BuiltInRegistries.BLOCK.getKey(block.getBlock()).toString());
        info.put("ore", NetherSafety.ore(block));
        info.put("clear", !exposed && !dangerous(block) && Passages.clear(mc,pos,block));
        info.put("support", Passages.supports(mc,pos,block));
        info.put("openable", !exposed && !dangerous(block) && Passages.canOpen(block));
        info.put("danger", exposed || dangerous(block));
        info.put("fluid", !block.getFluidState().isEmpty());
        info.put("water", block.getFluidState().is(net.minecraft.tags.FluidTags.WATER));
        info.put("falling", block.getBlock() instanceof net.minecraft.world.level.block.FallingBlock);
        info.put("diggable", !block.hasBlockEntity() && (isTerrain(block) || NetherSafety.ore(block)) &&
            correct && hardness >= 0 && !exposed && MiningProfile.mayMine(mc,pos) && !dangerous(block) && !supportsTorch(mc,pos) && NetherSafety.miningAllowed(mc,pos,block));
        // Preview the tool we will select, not whichever item a previous action left in hand.
        double seconds = ToolChoice.seconds(mc, pos);
        info.put("seconds", Double.isFinite(seconds) ? Math.min(3600, seconds) : 3600);
        return info;
    }

    private static void beginScan(Minecraft mc, String id) {
        scanCenter = mc.player.blockPosition();
        int bottom = Math.max(mc.level.getMinBuildHeight(), scanCenter.getY() - 16);
        int top = Math.min(mc.level.getMaxBuildHeight() - 1, scanCenter.getY() + 16);
        scanOrigin = new BlockPos(scanCenter.getX() - 16, bottom, scanCenter.getZ() - 16);
        scanHeight = top - bottom + 1;
        scanCells = new int[33 * 33 * scanHeight];
        scanCursor = 0;
        scanPalette.clear(); scanStates.clear();
        scanChests.clear();
        scanPalette.add(Map.of("block", "unknown"));
        activeId = id; remainingTicks = 200;
        result(id, "running", "Scanning loaded blocks within 16 blocks of the player.");
    }

    private static void updateScan(Minecraft mc) throws IOException {
        if (!mc.player.blockPosition().equals(scanCenter)) throw new IllegalStateException("Player moved during scan; retry.");
        // Slice world access across ticks, with both cell and wall-clock budgets.
        long deadline = System.nanoTime() + 3_000_000;
        int end = Math.min(scanCells.length, scanCursor + 4096);
        while (scanCursor < end) {
            int i = scanCursor++;
            BlockPos pos = scanOrigin.offset(i % 33, i / (33 * 33), (i / 33) % 33);
            if (mc.level.hasChunkAt(pos)) {
                if (HomeSettings.storageArea(mc, pos) && ChestTransfer.supported(mc, pos))
                    scanChests.add(Map.of("x", pos.getX(), "y", pos.getY(), "z", pos.getZ()));
                BlockState state = mc.level.getBlockState(pos);
                long key = MiningProfile.paletteKey(Block.getId(state),supportsTorch(mc,pos),!NetherSafety.miningAllowed(mc,pos,state),
                    MiningProfile.exposed(mc,pos),!MiningProfile.mayMine(mc,pos));
                Integer paletteId = scanStates.get(key);
                if (paletteId == null) {
                    paletteId = scanPalette.size(); scanStates.put(key, paletteId);
                    scanPalette.add(properties(mc, pos));
                }
                scanCells[i] = paletteId;
            }
            if ((i & 127) == 0 && System.nanoTime() >= deadline) break;
        }
        if (scanCursor < scanCells.length) return;
        Map<String, Object> scan = new LinkedHashMap<>();
        scan.put("protocol", 8); scan.put("id", activeId); scan.put("updatedAt", System.currentTimeMillis());
        scan.put("server", sessionId(mc)); scan.put("username", mc.player.getGameProfile().getName());
        scan.put("dimension", mc.level.dimension().location().toString());
        scan.put("origin", Map.of("x", scanOrigin.getX(), "y", scanOrigin.getY(), "z", scanOrigin.getZ()));
        scan.put("size", Map.of("x", 33, "y", scanHeight, "z", 33));
        scan.put("palette", scanPalette); scan.put("cells", scanCells);
        scan.put("chests", scanChests);
        Path temporary = DIRECTORY.resolve("scan.tmp");
        Files.writeString(temporary, JSON.toJson(scan), StandardCharsets.UTF_8);
        Files.move(temporary, DIRECTORY.resolve("scan.json"), StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
        stop(mc, "done", "Local scan ready.");
    }

    private static Map<String, Object> navigation(Minecraft mc) {
        BlockPos feet = StandingGeometry.feet(mc);
        var directions = new ArrayList<Map<String, Object>>();
        var routes = new ArrayList<Map<String, Object>>();
        for (Direction direction : Direction.Plane.HORIZONTAL) {
            BlockPos next = feet.relative(direction);
            var foot = describeBlock(mc, next);
            var head = describeBlock(mc, next.above());
            directions.add(Map.of("name", direction.getName(), "x", next.getX(), "y", next.getY(), "z", next.getZ(),
                "safeFloor", safeFloor(mc, next), "foot", foot, "head", head,
                "clear", Boolean.TRUE.equals(foot.get("clear")) && Boolean.TRUE.equals(head.get("clear"))));
            for (int rise = -1; rise <= 1; rise++) {
                BlockPos destination = next.above(rise);
                var blocks = clearance(feet, destination).stream().map(pos -> describeBlock(mc, pos)).toList();
                routes.add(Map.of("name", direction.getName(), "level", rise, "x", destination.getX(), "y", destination.getY(),
                    "z", destination.getZ(), "safeFloor", safeFloor(mc, destination), "blocks", blocks,
                    "floor", describeBlock(mc, destination.below()),
                    "clear", blocks.stream().allMatch(block -> Boolean.TRUE.equals(block.get("clear")))));
            }
        }
        var ores = new ArrayList<Map<String, Object>>();
        for (BlockPos pos : BlockPos.betweenClosed(feet.offset(-4, -2, -4), feet.offset(4, 4, 4))) {
            if (mc.level.hasChunkAt(pos) && NetherSafety.ore(mc.level.getBlockState(pos))) {
                var block = describeBlock(mc, pos);
                if (Boolean.TRUE.equals(block.get("mineable"))) ores.add(block);
            }
        }
        Map<String, Object> result = new LinkedHashMap<>();
        result.put("directions", directions); result.put("routes", routes); result.put("ores", ores);
        result.put("sourceFloor", describeBlock(mc, feet.below()));
        result.put("feet", Map.of("x", feet.getX(), "y", feet.getY(), "z", feet.getZ()));
        boolean originSafe = safeFloor(mc, feet);
        boolean centered = originSafe && Math.hypot(mc.player.getX() - feet.getX() - .5, mc.player.getZ() - feet.getZ() - .5) < .12;
        result.put("originSafe", originSafe); result.put("centered", centered);
        BlockPos anchor = centered ? feet : findAnchor(mc);
        if (anchor != null) result.put("anchor", Map.of("x", anchor.getX(), "y", anchor.getY(), "z", anchor.getZ()));
        return result;
    }

    private static void updateAction(Minecraft mc) throws IOException {
        // A stalled/crashed controller cannot leave a movement/mining key held.
        Path heartbeat = DIRECTORY.resolve("heartbeat");
        if (System.currentTimeMillis() > commandDeadline && (!Files.isRegularFile(heartbeat) ||
            System.currentTimeMillis() - Files.getLastModifiedTime(heartbeat).toMillis() > 3000)) {
            stop(mc, "stopped", "Controller heartbeat expired."); return;
        }
        if (remainingTicks-- <= 0) {
            boolean completedStep = walkingKey != null && moveTarget == null && centerTarget == null;
            stop(mc, completedStep ? "done" : "error", completedStep ? "Step finished." : "Action timed out.");
            return;
        }
        if (!BreathEscape.active() && scanCells == null && enemy == null && !eating)
            MiningProfile.requireCover(mc);
        if (ChestTransfer.active()) {
            String completed = ChestTransfer.update(mc);
            if (completed != null) stop(mc, "done", completed);
            return;
        }
        if (PickaxeCraft.active()) {
            String completed=PickaxeCraft.update(mc);
            if(completed!=null)stop(mc,"done",completed);
            return;
        }
        if (Passages.active()) {
            String completed=Passages.update(mc);
            if(completed!=null)stop(mc,"done",completed);
            return;
        }
        if (scanCells != null) { updateScan(mc); return; }
        if (RecoveryMove.active()) {
            String complete=RecoveryMove.update(mc);
            if(complete!=null)stop(mc,"done",complete);
            return;
        }
        if (BreathEscape.active()) {
            String complete=BreathEscape.update(mc);
            if(complete!=null)stop(mc,"done",complete);
            return;
        }
        if(AirSupply.urgent(mc)) {stop(mc,"interrupted","Oxygen recovery preempts the current action.");return;}
        if (enemy==null && !eating && !RecoveryMove.active() && walkDirection==null)
            NetherSafety.requireWorkSafe(mc);
        if (centerTarget != null) { updateCenter(mc); return; }
        if (torchTarget != null) {
            if ((mc.level.getBlockState(torchTarget).is(Blocks.TORCH) || mc.level.getBlockState(torchTarget).is(Blocks.WALL_TORCH)) && ++changedTicks >= 2)
                stop(mc, "done", "Torch placed.");
            return;
        }
        if (moveTarget != null) { updateTraverse(mc); return; }
        if (eating) {
            if (mc.player.getFoodData().getFoodLevel() > foodBefore) { stop(mc, "done", "Food restored."); return; }
            mc.options.keyUse.setDown(true);
            return;
        }
        if (enemy != null) {
            if (!enemy.isAlive() || enemy.isRemoved()) { stop(mc,"done","Enemy is gone."); return; }
            validateAttack(mc, enemy);
            if (!matches(mc,mc.player.getMainHandItem(),"sword")) throw new IllegalStateException("Sword not equipped; retry after inventory sync.");
            if (mc.player.getAttackStrengthScale(0.5F) >= 0.9F) {
                if(!CombatSafety.safeSwordTick(mc,enemy))return;
                Vec3 delta = enemy.getEyePosition().subtract(mc.player.getEyePosition());
                mc.player.setYRot((float) (Math.toDegrees(Math.atan2(delta.z, delta.x)) - 90));
                mc.player.setXRot((float) -Math.toDegrees(Math.atan2(delta.y, Math.hypot(delta.x, delta.z))));
                mc.gameMode.attack(mc.player, enemy);
                mc.player.swing(InteractionHand.MAIN_HAND);
                stop(mc, "done", "Sword attack sent.");
            }
            return;
        }
        if (target != null) {
            if (!mc.level.getBlockState(target).is(originalBlock)) {
                if (++changedTicks >= 2) stop(mc, "done", "Block changed for two ticks; pickup count is separate.");
                return;
            }
            changedTicks = 0;
            validateMine(mc, target);
            mc.gameMode.continueDestroyBlock(target, targetFace);
            mc.player.swing(InteractionHand.MAIN_HAND);
        } else if (walkingKey != null) {
            validateStep(mc, walkDirection);
            walkingKey.setDown(true);
        }
    }

    static void stop(Minecraft mc, String status, String message) {
        ChestTransfer.cancel(mc);
        PickaxeCraft.cancel(mc);
        Passages.cancel();
        RecoveryMove.cancel(mc);
        BreathEscape.cancel(mc);
        if (eating) { mc.options.keyUse.setDown(false); if (mc.gameMode != null && mc.player != null) mc.gameMode.releaseUsingItem(mc.player); }
        if (walkingKey != null) walkingKey.setDown(false);
        if (moveTarget != null) mc.options.keyJump.setDown(false);
        if (target != null && mc.gameMode != null) mc.gameMode.stopDestroyBlock();
        if (activeId != null) result(activeId, status, message);
        walkingKey = null;
        moveOrigin = null;
        moveTarget = null;
        moveQueue.clear();
        if (mc.player != null) mc.player.setSprinting(false);
        jumpStarted = false;
        walkDirection = null;
        target = null;
        originalBlock = null;
        activeId = null;
        eating = false;
        enemy = null;
        torchTarget = null;
        centerTarget = null;
        scanCells = null; scanOrigin = null; scanCenter = null;
        scanPalette.clear(); scanStates.clear();
        remainingTicks = 0;
        travelling = false; returning = false;
        combatApproach=false;
    }

    private static String actionName() { return BreathEscape.active() ? "breathe" : RecoveryMove.active() ? "escape" : Passages.active() ? "open_passage" : PickaxeCraft.active() ? "craft_pickaxe" : ChestTransfer.active() ? "store" : centerTarget != null ? "center" : torchTarget != null ? "torch" : scanCells != null ? "scan" : target != null ? "mine" : moveTarget != null ? "traverse" : walkingKey != null ? "step" : eating ? "eat" : enemy != null ? "attack" : "idle"; }

    private static void result(String id, String status, String message) {
        lastId = id;
        lastStatus = status;
        lastMessage = message == null ? "Command failed." : message;
    }

    static void writeState(Minecraft mc) {
        try { writeStateChecked(mc); }
        catch (RuntimeException error) {
            LOGGER.warn("Flyminer state inspection failed; releasing inputs",error);
            stop(mc,"error","State inspection failed: "+error.getClass().getSimpleName());
        }
    }
    private static void writeStateChecked(Minecraft mc) {
        Map<String, Object> state = new LinkedHashMap<>();
        state.put("updatedAt", System.currentTimeMillis());
        state.put("bridgeVersion", "0.11.0");
        state.put("capabilities", java.util.List.of("hazard_recovery","rear_torches","nether_mining","oxygen_escape","fractional_floor","block_tools","mining_profile"));
        state.put("protocol", 8);
        state.put("miningTarget", miningTarget);
        state.put("controller", "local bridge; controller identity is in the external run log");
        state.put("enabled", enabled);
        state.put("expectedSession", expectedSession(mc));
        state.put("screenOpen", mc.screen != null);
        state.put("containerOwned", ChestTransfer.owned(mc)||PickaxeCraft.owned(mc));
        state.put("action", actionName());
        state.put("lastResult", Map.of("id", lastId, "status", lastStatus, "message", lastMessage));
        if (mc.player != null && mc.level != null) {
            state.put("home", HomeSettings.get(mc));
            state.put("oxygen", AirSupply.state(mc));
            state.put("miningProfile", MiningProfile.state(mc));
            state.put("username", mc.player.getGameProfile().getName());
            state.put("server", sessionId(mc));
            state.put("position", Map.of("x", mc.player.getX(), "y", mc.player.getY(), "z", mc.player.getZ()));
            state.put("yaw", mc.player.getYRot());
            state.put("pitch", mc.player.getXRot());
            state.put("health", mc.player.getHealth());
            state.put("food", mc.player.getFoodData().getFoodLevel());
            state.put("onGround", mc.player.onGround());
            state.put("inWater", mc.player.isInWater());
            state.put("inLava", mc.player.isInLava());
            state.put("onFire", mc.player.isOnFire());
            state.put("fireResistant", mc.player.hasEffect(net.minecraft.world.effect.MobEffects.FIRE_RESISTANCE));
            state.put("liquidSafe", LiquidSafety.bodyClear(mc,mc.player.getBoundingBox()));
            state.put("lightLevel", mc.level.getMaxLocalRawBrightness(mc.player.blockPosition()));
            state.put("freeSlots", mc.player.getInventory().items.stream().filter(ItemStack::isEmpty).count());
            state.put("horizontalCollision", mc.player.horizontalCollision);
            state.put("dimension", mc.level.dimension().location().toString());
            state.put("biome", mc.level.getBiome(mc.player.blockPosition()).unwrapKey().map(key -> key.location().toString()).orElse("unknown"));
            state.put("heldItem", BuiltInRegistries.ITEM.getKey(mc.player.getMainHandItem().getItem()).toString());
            state.put("targetHarvestable", canHarvestTarget(mc));
            state.put("crafting", PickaxeCraft.preview(mc));
            var inventory = new ArrayList<Map<String, Object>>();
            for (var stack : mc.player.getInventory().items) {
                if (!stack.isEmpty()) inventory.add(Map.of("item", BuiltInRegistries.ITEM.getKey(stack.getItem()).toString(), "count", stack.getCount(),
                    "pickaxe", matches(mc, stack, "pickaxe"), "sword", matches(mc, stack, "sword"), "food", matches(mc, stack, "food"),
                    "torch", matches(mc, stack, "torch"),
                    "craftingReserve", PickaxeCraft.keepMaterial(mc,stack),
                    "harvestsTarget", harvestsTarget(mc, stack),
                    "durability", stack.isDamageableItem() ? stack.getMaxDamage() - stack.getDamageValue() : -1));
            }
            state.put("inventory", inventory);
            state.put("navigation", navigation(mc));
            var hostiles = new ArrayList<Map<String, Object>>();
            for (LivingEntity hostile : mc.level.getEntitiesOfClass(LivingEntity.class, mc.player.getBoundingBox().inflate(NetherSafety.active(mc)?32:10),
                entity -> entity.isAlive() && !entity.isAlliedTo(mc.player) && (hostileToPlayer(mc, entity) || NetherSafety.active(mc) && NetherSafety.avoidOnly(entity)))) {
                var info = new LinkedHashMap<String,Object>(Map.of("id", hostile.getId(), "uuid", hostile.getUUID().toString(),
                    "type", BuiltInRegistries.ENTITY_TYPE.getKey(hostile.getType()).toString(),
                    "x", hostile.getX(), "y", hostile.getY(), "z", hostile.getZ(), "health", hostile.getHealth(),
                    "distance", mc.player.distanceTo(hostile), "visible", mc.player.hasLineOfSight(hostile),
                    "exploding", hostile instanceof net.minecraft.world.entity.monster.Creeper creeper && creeper.getSwellDir() > 0));
                info.put("attackDistance",attackDistance(mc,hostile));
                info.put("ranged",NetherSafety.ranged(hostile));
                info.put("avoidOnly",NetherSafety.active(mc)&&NetherSafety.avoidOnly(hostile));
                hostiles.add(info);
            }
            state.put("hostiles", hostiles);
            if (mc.hitResult instanceof BlockHitResult hit && hit.getType() == HitResult.Type.BLOCK) {
                BlockPos pos = hit.getBlockPos();
                state.put("crosshairBlock", describeBlock(mc, pos));
            }
        }
        try {
            Path temporary = DIRECTORY.resolve("state.tmp");
            Files.writeString(temporary, JSON.toJson(state), StandardCharsets.UTF_8);
            Files.move(temporary, DIRECTORY.resolve("state.json"), StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
        } catch (IOException error) {
            if (ticks % 200 == 0) LOGGER.warn("Unable to write local flyminer state: {}", error.getMessage());
        }
    }
}
