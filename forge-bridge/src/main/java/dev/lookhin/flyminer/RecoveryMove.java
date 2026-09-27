package dev.lookhin.flyminer;

import net.minecraft.client.Minecraft;
import net.minecraft.core.BlockPos;
import net.minecraft.world.phys.AABB;
import java.util.ArrayList;
import java.util.Comparator;

final class RecoveryMove {
    private static BlockPos destination;
    private static boolean avoidThreats;
    static boolean active(){return destination!=null;}
    static void cancel(Minecraft mc){destination=null;mc.options.keyUp.setDown(false);}
    private static boolean safe(Minecraft mc,BlockPos to) {
        if(!mc.player.onGround() || mc.player.isInWater() || mc.player.isInLava() || mc.player.isPassenger() || mc.player.getHealth()<=8) return false;
        var floor=mc.level.getBlockState(to.below());
        if(!Passages.supports(mc,to.below(),floor) || floor.getBlock() instanceof net.minecraft.world.level.block.FallingBlock) return false;
        AABB from=mc.player.getBoundingBox();
        AABB end=from.move(to.getX()+.5-mc.player.getX(),0,to.getZ()+.5-mc.player.getZ());
        var threats=avoidThreats?NetherSafety.threats(mc).stream().filter(e->mc.player.hasLineOfSight(e)).toList():java.util.List.<net.minecraft.world.entity.LivingEntity>of();
        if(avoidThreats && (threats.isEmpty() || threats.stream().anyMatch(e->!NetherSafety.farther(from,end,e.position(),true))))return false;
        return RecoverySafety.checked(from,end,
            p->mc.level.hasChunkAt(p)&&mc.level.getFluidState(p).isEmpty(),
            box->ClientBridge.supportedDryBody(mc,box) && (avoidThreats
                ? threats.stream().allMatch(e->NetherSafety.farther(from,box,e.position(),false))
                : mc.level.getEntitiesOfClass(net.minecraft.world.entity.LivingEntity.class,box.inflate(3.5),
                    e->ClientBridge.hostileToPlayer(mc,e)&&mc.player.hasLineOfSight(e)).isEmpty()));
    }
    static boolean begin(Minecraft mc, boolean threats) {
        avoidThreats=threats;
        var feet=StandingGeometry.feet(mc);
        var candidates=new ArrayList<BlockPos>();
        for(int x=-2;x<=2;x++)for(int z=-2;z<=2;z++)candidates.add(feet.offset(x,0,z));
        candidates.sort(Comparator.comparingDouble(p->mc.player.position().distanceToSqr(p.getX()+.5,mc.player.getY(),p.getZ()+.5)));
        for(BlockPos p:candidates)if(safe(mc,p)){destination=p;return true;}
        return false;
    }
    static String update(Minecraft mc) {
        double dx=destination.getX()+.5-mc.player.getX(),dz=destination.getZ()+.5-mc.player.getZ();
        double distance=Math.hypot(dx,dz),speed=mc.player.getDeltaMovement().horizontalDistance();
        if(distance<.10&&speed<.025 && mc.player.onGround() && ClientBridge.supportedDryBody(mc,mc.player.getBoundingBox())
            && LiquidSafety.bodyClear(mc,mc.player.getBoundingBox()))return "Retreated onto supported ground with the full fluid buffer.";
        if(!safe(mc,destination))throw new IllegalStateException("Retreat corridor changed; recheck another dry escape.");
        mc.player.setYRot((float)Math.toDegrees(Math.atan2(-dx,dz)));
        mc.player.setXRot(0);
        mc.player.setSprinting(false);
        mc.options.keyUp.setDown(distance>Math.max(.06,speed*2.2+.04));
        return null;
    }
}
