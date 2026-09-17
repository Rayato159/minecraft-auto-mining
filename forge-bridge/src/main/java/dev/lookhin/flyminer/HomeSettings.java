package dev.lookhin.flyminer;

import com.google.gson.Gson;
import com.google.gson.reflect.TypeToken;
import com.mojang.brigadier.arguments.IntegerArgumentType;
import net.minecraft.client.Minecraft;
import net.minecraft.commands.Commands;
import net.minecraft.core.BlockPos;
import net.minecraftforge.client.event.RegisterClientCommandsEvent;
import net.minecraftforge.fml.loading.FMLPaths;
import java.io.IOException;
import java.nio.file.*;
import java.util.LinkedHashMap;
import java.util.Map;

/** Settings are isolated by world/server, player and dimension; protection never depends on Rust. */
final class HomeSettings {
    private static final Gson JSON = new Gson();
    private static final Path FILE = FMLPaths.CONFIGDIR.get().resolve("flyminer/home.json");
    private static Map<String, Profile> profiles = new LinkedHashMap<>();
    record Point(int x, int y, int z) {
        BlockPos pos() { return new BlockPos(x, y, z); }
        static Point of(BlockPos p) { return new Point(p.getX(), p.getY(), p.getZ()); }
    }
    static final class Profile {
        Point home, mine;
        int radius = 16, searchRadius = 8;
        long revision, request, acknowledged;
    }
    static void load() throws IOException {
        if (!Files.exists(FILE)) return;
        try {
            profiles = JSON.fromJson(Files.readString(FILE), new TypeToken<Map<String, Profile>>() {}.getType());
            if (profiles == null || profiles.values().stream().anyMatch(p -> p == null || p.radius < 1 ||
                    p.radius > 256 || p.searchRadius < 1 || p.searchRadius > 16))
                throw new IllegalArgumentException("Invalid home settings");
        } catch (RuntimeException e) { throw new IOException("Cannot read home protection settings; fix home.json before enabling", e); }
    }
    static Profile get(Minecraft mc) {
        String key = ClientBridge.sessionId(mc) + "|" + mc.player.getGameProfile().getName() + "|" + mc.level.dimension().location();
        return profiles.computeIfAbsent(key, ignored -> new Profile());
    }
    static boolean protectedAt(Minecraft mc, BlockPos p) {
        Profile v = get(mc);
        if (v.home == null) return false;
        long dx = (long) p.getX() - v.home.x(), dz = (long) p.getZ() - v.home.z();
        return dx * dx + dz * dz <= (long) v.radius * v.radius;
    }
    static boolean storageArea(Minecraft mc, BlockPos p) {
        Profile v = get(mc);
        return v.home != null && v.home.pos().distSqr(p) <= (double) v.searchRadius * v.searchRadius;
    }
    static void save() throws IOException {
        Path temp = FILE.resolveSibling("home.tmp");
        Files.writeString(temp, JSON.toJson(profiles));
        Files.move(temp, FILE, StandardCopyOption.ATOMIC_MOVE, StandardCopyOption.REPLACE_EXISTING);
    }
    static void acknowledge(Minecraft mc, long request) throws IOException {
        Profile p = get(mc);
        if (request != p.request) throw new IllegalStateException("Home request changed; retry");
        p.acknowledged = request;
        save();
    }
    private static int edit(java.util.function.Consumer<Profile> change, String message) {
        Minecraft mc = Minecraft.getInstance();
        if (!ClientBridge.expectedSession(mc)) { ClientBridge.localMessage("Join your world first."); return 0; }
        ClientBridge.stop(mc, "interrupted", "Home settings/request changed.");
        Profile p = get(mc);
        change.accept(p);
        p.revision = Math.max(p.revision + 1, System.currentTimeMillis());
        try { save(); }
        catch (IOException e) { ClientBridge.localMessage("Settings active but could not save: " + e.getMessage()); return 0; }
        ClientBridge.writeState(mc);
        ClientBridge.localMessage(message);
        return 1;
    }
    static void register(RegisterClientCommandsEvent event) {
        var home = Commands.literal("home").executes(c -> {
            Minecraft mc = Minecraft.getInstance();
            if (!ClientBridge.expectedSession(mc) || get(mc).home == null) {
                ClientBridge.localMessage("Set /flyminer home set near your chest first."); return 0;
            }
            return edit(p -> p.request = Math.max(p.request + 1, System.currentTimeMillis()),
                "Return requested. Close chat; the running Rust controller will deposit/refill then resume at the mine point.");
        });
        home.then(pointCommand("set", true));
        home.then(Commands.literal("radius").then(Commands.argument("blocks", IntegerArgumentType.integer(1, 256))
            .executes(c -> edit(p -> p.radius = IntegerArgumentType.getInteger(c, "blocks"), "Home no-dig radius updated (all heights)."))));
        home.then(Commands.literal("search").then(Commands.argument("blocks", IntegerArgumentType.integer(1, 16))
            .executes(c -> edit(p -> p.searchRadius = IntegerArgumentType.getInteger(c, "blocks"), "Chest search radius updated."))));
        home.then(Commands.literal("status").executes(c -> {
            Minecraft mc = Minecraft.getInstance();
            if (!ClientBridge.expectedSession(mc)) return 0;
            ClientBridge.localMessage(JSON.toJson(get(mc))); return 1;
        }));
        home.then(Commands.literal("clear").executes(c -> edit(p -> {
            p.home = null; p.acknowledged = p.request;
        }, "Home cleared; automatic return and home protection disabled.")));
        event.getDispatcher().register(Commands.literal("flyminer").then(home)
            .then(Commands.literal("mine").then(pointCommand("set", false))));
    }
    private static com.mojang.brigadier.builder.LiteralArgumentBuilder<net.minecraft.commands.CommandSourceStack> pointCommand(String name, boolean home) {
        return Commands.literal(name).executes(c -> setPoint(home, Minecraft.getInstance().player.blockPosition()))
            .then(Commands.argument("x", IntegerArgumentType.integer(-30000000, 30000000))
                .then(Commands.argument("y", IntegerArgumentType.integer(-2048, 2048))
                    .then(Commands.argument("z", IntegerArgumentType.integer(-30000000, 30000000))
                        .executes(c -> setPoint(home, new BlockPos(IntegerArgumentType.getInteger(c, "x"),
                            IntegerArgumentType.getInteger(c, "y"), IntegerArgumentType.getInteger(c, "z")))))));
    }
    private static int setPoint(boolean home, BlockPos pos) {
        return edit(p -> { if (home) p.home = Point.of(pos); else p.mine = Point.of(pos); },
            (home ? "Home/chest area" : "Mine standing point") + " set to " + pos.toShortString() + ".");
    }
}
