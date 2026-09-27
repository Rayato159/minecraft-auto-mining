package dev.lookhin.flyminer;

import net.minecraft.client.Minecraft;
import net.minecraft.core.BlockPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.tags.BlockTags;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.entity.monster.*;
import net.minecraft.world.entity.monster.piglin.Piglin;
import net.minecraft.world.level.Level;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.FallingBlock;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.AABB;
import java.util.Set;
import java.util.function.Predicate;

/** Conservative Nether rules, rechecked on the client thread before acting. */
final class NetherSafety {
    static final Set<String> ROCKS = Set.of("minecraft:netherrack", "minecraft:basalt",
        "minecraft:smooth_basalt", "minecraft:blackstone", "minecraft:soul_soil", "minecraft:soul_sand");
    static boolean active(Minecraft mc) { return mc.level != null && mc.level.dimension().equals(Level.NETHER); }
    static boolean terrain(BlockState block) { return ROCKS.contains(BuiltInRegistries.BLOCK.getKey(block.getBlock()).toString()); }
    static boolean ore(BlockState block) {
        return oreId(BuiltInRegistries.BLOCK.getKey(block.getBlock()).toString(),block.is(net.minecraftforge.common.Tags.Blocks.ORES));
    }
    static boolean oreId(String id, boolean tagged) { return tagged || id.equals("minecraft:ancient_debris"); }
    // Ordinary piglins and zombified piglins are never proactively attacked.
    // Client-side anger/target data is not a reliable authority for neutral mobs.
    static boolean avoidOnly(LivingEntity e) { return e instanceof Piglin || e instanceof ZombifiedPiglin; }
    static boolean ranged(LivingEntity e) {
        return e instanceof Ghast || e instanceof Blaze || e instanceof AbstractSkeleton
            || e instanceof net.minecraft.world.entity.boss.wither.WitherBoss;
    }
    static double radius(LivingEntity e) { return ranged(e) ? 24 : avoidOnly(e) ? 8 : 4; }
    static java.util.List<LivingEntity> threats(Minecraft mc) {
        if (!active(mc)) return java.util.List.of();
        return mc.level.getEntitiesOfClass(LivingEntity.class,mc.player.getBoundingBox().inflate(32),
            e -> e.isAlive() && !e.isAlliedTo(mc.player) && (avoidOnly(e) || ClientBridge.hostileToPlayer(mc,e)));
    }
    static boolean interrupts(Minecraft mc) {
        if (!active(mc)) return false;
        return mc.player.isOnFire() && !mc.player.hasEffect(net.minecraft.world.effect.MobEffects.FIRE_RESISTANCE)
            || threats(mc).stream().anyMatch(e -> (ranged(e)||avoidOnly(e)) && mc.player.hasLineOfSight(e)
                && mc.player.distanceTo(e)<radius(e));
    }
    static void requireWorkSafe(Minecraft mc) {
        if (interrupts(mc)) throw new IllegalStateException("Nether danger: fire, ranged enemy or nearby piglin; retreat before working.");
    }
    static boolean piglinsNearby(Minecraft mc, BlockPos pos) {
        return active(mc) && !mc.level.getEntitiesOfClass(Piglin.class,new AABB(pos).inflate(16),LivingEntity::isAlive).isEmpty();
    }
    static boolean guardedGold(BlockState block) {
        return block.is(BlockTags.GUARDED_BY_PIGLINS) || block.is(Blocks.NETHER_GOLD_ORE)
            || block.is(Blocks.GILDED_BLACKSTONE) || block.is(Blocks.GOLD_ORE) || block.is(Blocks.DEEPSLATE_GOLD_ORE);
    }
    static boolean miningAllowed(Minecraft mc, BlockPos pos, BlockState block) {
        return !guardedGold(block) || !piglinsNearby(mc,pos);
    }
    static boolean roof(BlockPos feet, Predicate<BlockPos> stable) {
        return stable.test(feet.above(2)) || stable.test(feet.above(3));
    }
    static boolean coveredAfterCut(Minecraft mc, BlockPos cut) {
        return roof(StandingGeometry.feet(mc), p -> {
            if(p.equals(cut)||!mc.level.hasChunkAt(p))return false;
            BlockState b=mc.level.getBlockState(p);
            return b.getFluidState().isEmpty() && !(b.getBlock() instanceof FallingBlock)
                && !b.is(BlockTags.FIRE) && !b.is(Blocks.MAGMA_BLOCK) && Passages.supports(mc,p,b);
        });
    }
    // Testable geometric rule used for the entire recovery sweep, not only its endpoint.
    static boolean farther(AABB from, AABB to, net.minecraft.world.phys.Vec3 enemy, boolean endpoint) {
        double before=from.getCenter().distanceToSqr(enemy), after=to.getCenter().distanceToSqr(enemy);
        return after + 1e-5 >= before && (!endpoint || after > before + .0001);
    }
}
