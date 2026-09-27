package dev.lookhin.flyminer;

import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import java.util.*;

/** Geometry only; Minecraft still checks support, reach, collision and the hit face. */
final class TorchPlacement {
    record Spot(BlockPos position, BlockPos support, Direction face) {}
    static List<Spot> candidates(BlockPos feet, int dx, int dz, Set<BlockPos> avoid) {
        if (Math.abs(dx) + Math.abs(dz) != 1) throw new IllegalArgumentException("Torch heading must be cardinal.");
        List<Spot> spots = new ArrayList<>();
        for (int x=-3; x<=3; x++) for (int z=-3; z<=3; z++) for (int y=0; y<=2; y++) {
            if (x*dx + z*dz >= 0 || x*x+z*z > 10) continue;
            BlockPos pos = feet.offset(x,y,z);
            if (avoid.contains(pos)) continue;
            for (Direction face : new Direction[]{Direction.NORTH,Direction.SOUTH,Direction.EAST,Direction.WEST,Direction.UP}) {
                BlockPos support = pos.relative(face.getOpposite());
                if (avoid.contains(support) || (support.getX()-feet.getX())*dx+(support.getZ()-feet.getZ())*dz >= 0) continue;
                spots.add(new Spot(pos,support,face));
            }
        }
        spots.sort(Comparator.<Spot>comparingInt(s -> s.face()==Direction.UP ? 1 : 0)
            .thenComparingInt(s -> Math.abs(s.position().getY()-feet.getY()-1))
            .thenComparingDouble(s -> s.position().distSqr(feet)));
        return spots;
    }
}
