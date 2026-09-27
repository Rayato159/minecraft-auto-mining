package dev.lookhin.flyminer;

import net.minecraft.core.BlockPos;
import net.minecraft.world.phys.AABB;
import java.util.ArrayList;
import java.util.function.Predicate;

/** Exception for retreat only: never approach a liquid/unknown cell more closely.
 * The destination regains the full buffer. Every swept body must stay dry,
 * collision-free and supported; there is no digging, jumping or dropping. */
final class RecoverySafety {
    private static double gap(AABB box, BlockPos p) {
        double dx=Math.max(0,Math.max(p.getX()-box.maxX,box.minX-(p.getX()+1.)));
        double dy=Math.max(0,Math.max(p.getY()-box.maxY,box.minY-(p.getY()+1.)));
        double dz=Math.max(0,Math.max(p.getZ()-box.maxZ,box.minZ-(p.getZ()+1.)));
        return Math.max(dx,Math.max(dy,dz));
    }
    static boolean checked(AABB from, AABB to, Predicate<BlockPos> knownDry, Predicate<AABB> supportedDry) {
        double dx=to.minX-from.minX, dz=to.minZ-from.minZ;
        if (Math.abs(to.minY-from.minY)>.001 || Math.hypot(dx,dz)>2.6 || !LiquidSafety.boxClear(to,knownDry)) return false;
        AABB area=from.minmax(to).inflate(LiquidSafety.RADIUS+.001);
        var hazards=new ArrayList<BlockPos>();
        for (BlockPos p:BlockPos.betweenClosed(BlockPos.containing(area.minX,area.minY,area.minZ),
                BlockPos.containing(area.maxX,area.maxY,area.maxZ))) {
            if (!knownDry.test(p)) hazards.add(p.immutable());
        }
        double[] previous=new double[hazards.size()];
        for (int j=0;j<previous.length;j++) previous[j]=Math.min(LiquidSafety.RADIUS,gap(from,hazards.get(j)));
        int slices=Math.max(1,(int)Math.ceil(Math.hypot(dx,dz)/.06));
        for(int i=0;i<=slices;i++) {
            AABB body=from.move(dx*i/slices,0,dz*i/slices);
            if(!supportedDry.test(body))return false;
            for(int j=0;j<previous.length;j++) {
                double clearance=Math.min(LiquidSafety.RADIUS,gap(body,hazards.get(j)));
                if(clearance+.00001<previous[j])return false;
                previous[j]=clearance;
            }
        }
        return true;
    }
}
