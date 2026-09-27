package dev.lookhin.flyminer;

import net.minecraft.client.Minecraft;
import net.minecraft.core.BlockPos;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.phys.Vec3;

/** Route cells describe the space above a floor, not the block containing sunken feet. */
final class StandingGeometry {
    static BlockPos cell(Vec3 position, boolean grounded, boolean inSoulSand) {
        BlockPos raw = BlockPos.containing(position);
        return grounded && inSoulSand && Math.abs(position.y - raw.getY() - .875) < .025
            ? raw.above() : raw;
    }
    static BlockPos feet(Minecraft mc) {
        return cell(mc.player.position(), mc.player.onGround(),
            mc.level.getBlockState(mc.player.blockPosition()).is(Blocks.SOUL_SAND));
    }
    static double floorY(BlockPos feet, boolean soulSand) {
        return feet.getY() - (soulSand ? .125 : 0.);
    }
    static double floorY(Minecraft mc, BlockPos feet) {
        return floorY(feet, mc.level.getBlockState(feet.below()).is(Blocks.SOUL_SAND));
    }
    static boolean atHeight(double playerY, double floorY) {
        return Math.abs(playerY - floorY) < .04;
    }
    static boolean landed(Minecraft mc, BlockPos feet) {
        return mc.player.onGround() && atHeight(mc.player.getY(), floorY(mc, feet))
            && ClientBridge.supportedDryBody(mc, mc.player.getBoundingBox());
    }
}
