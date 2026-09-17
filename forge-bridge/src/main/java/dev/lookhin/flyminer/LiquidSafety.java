package dev.lookhin.flyminer;

import net.minecraft.client.Minecraft;
import net.minecraft.core.BlockPos;
import net.minecraft.world.phys.AABB;
import java.util.function.Predicate;

/** Two full cells in every direction, including diagonals and vertical neighbors. */
final class LiquidSafety {
    static final int RADIUS = 2;
    static boolean cubeClear(BlockPos center, Predicate<BlockPos> knownDry) {
        var cursor = new BlockPos.MutableBlockPos();
        for (int y=-RADIUS; y<=RADIUS; y++) for (int z=-RADIUS; z<=RADIUS; z++) for (int x=-RADIUS; x<=RADIUS; x++) {
            cursor.set(center.getX()+x, center.getY()+y, center.getZ()+z);
            if (!knownDry.test(cursor)) return false;
        }
        return true;
    }
    static boolean cellClear(Minecraft mc, BlockPos pos) {
        try { return cubeClear(pos, p -> mc.level.hasChunkAt(p) && mc.level.getFluidState(p).isEmpty()); }
        catch (RuntimeException error) { return false; }
    }
    static boolean bodyClear(Minecraft mc, BlockPos feet) {
        return cellClear(mc,feet) && cellClear(mc,feet.above());
    }
    static boolean boxClear(AABB box, Predicate<BlockPos> knownDry) {
        // Include the real body while it overlaps two cells or is between stair heights.
        AABB margin = box.inflate(RADIUS).deflate(.00001);
        for (BlockPos p : BlockPos.betweenClosed(BlockPos.containing(margin.minX,margin.minY,margin.minZ),
                BlockPos.containing(margin.maxX,margin.maxY,margin.maxZ))) {
            if (!knownDry.test(p)) return false;
        }
        return true;
    }
    static boolean bodyClear(Minecraft mc, AABB box) {
        try { return boxClear(box, p -> mc.level.hasChunkAt(p) && mc.level.getFluidState(p).isEmpty()); }
        catch (RuntimeException error) { return false; }
    }
    static void requirePlayerClear(Minecraft mc) {
        if (!bodyClear(mc,mc.player.getBoundingBox()))
            throw new IllegalStateException("Water/lava or unknown terrain within two blocks of the player.");
    }
}
