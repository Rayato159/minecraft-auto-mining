package dev.lookhin.flyminer;

import net.minecraft.client.Minecraft;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.level.ClipContext;
import net.minecraft.world.level.block.DoorBlock;
import net.minecraft.world.level.block.FenceGateBlock;
import net.minecraft.world.level.block.StairBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.HitResult;
import net.minecraft.world.phys.Vec3;
import net.minecraft.world.phys.shapes.VoxelShape;

/** Walking geometry and ordinary hand-operated passages; never trapdoors or block breaking. */
final class Passages {
    private static BlockPos target;
    private static int age, stable;
    static boolean active() { return target != null; }
    static void cancel() { target = null; }
    static boolean canOpen(BlockState state) {
        return state.getBlock() instanceof DoorBlock door && door.type().canOpenByHand() && !state.getValue(DoorBlock.OPEN)
            || state.getBlock() instanceof FenceGateBlock && !state.getValue(FenceGateBlock.OPEN);
    }
    static boolean centerEmpty(VoxelShape shape) {
        var body = new AABB(.2,0,.2,.8,1,.8);
        return shape.toAabbs().stream().noneMatch(box -> box.intersects(body));
    }
    static boolean stairTop(VoxelShape shape) {
        // Only surfaces at the integer standing height: never a fence top or a half slab.
        return !shape.isEmpty() && Math.abs(shape.max(Direction.Axis.Y)-1.)<.00001 &&
            shape.toAabbs().stream().anyMatch(b -> Math.abs(b.maxY-1.)<.00001 &&
                Math.min(b.maxX,.8)-Math.max(b.minX,.2)>=.15 && Math.min(b.maxZ,.8)-Math.max(b.minZ,.2)>=.15);
    }
    static boolean supports(Minecraft mc, BlockPos pos, BlockState state) {
        return state.isFaceSturdy(mc.level,pos,Direction.UP) ||
            state.getBlock() instanceof StairBlock && stairTop(state.getCollisionShape(mc.level,pos));
    }
    static boolean clear(Minecraft mc, BlockPos pos, BlockState state) {
        var shape=state.getCollisionShape(mc.level,pos);
        return shape.isEmpty() || state.getBlock() instanceof DoorBlock && state.getValue(DoorBlock.OPEN) && centerEmpty(shape);
    }
    static void begin(Minecraft mc, BlockPos pos) {
        if(!mc.level.hasChunkAt(pos)||!canOpen(mc.level.getBlockState(pos)))throw new IllegalStateException("Passage changed or cannot be opened by hand.");
        var eye=mc.player.getEyePosition();var point=Vec3.atCenterOf(pos);
        if(eye.distanceTo(point)>Math.min(4.5,mc.gameMode.getPickRange()))throw new IllegalStateException("Passage is out of reach.");
        var hit=mc.level.clip(new ClipContext(eye,point,ClipContext.Block.OUTLINE,ClipContext.Fluid.NONE,mc.player));
        if(hit.getType()!=HitResult.Type.BLOCK||!hit.getBlockPos().equals(pos))throw new IllegalStateException("Passage is hidden behind a block.");
        mc.player.setShiftKeyDown(false);
        if(!mc.gameMode.useItemOn(mc.player,InteractionHand.MAIN_HAND,hit).consumesAction())throw new IllegalStateException("Game refused to open passage.");
        target=pos.immutable();age=0;stable=0;
    }
    static String update(Minecraft mc) {
        if(++age>60)throw new IllegalStateException("Passage did not open; it may be locked or controlled by redstone.");
        var state=mc.level.getBlockState(target);
        if(clear(mc,target,state) && ++stable>=4)return "Passage opened without breaking blocks.";
        if(!clear(mc,target,state))stable=0;
        return null;
    }
}
