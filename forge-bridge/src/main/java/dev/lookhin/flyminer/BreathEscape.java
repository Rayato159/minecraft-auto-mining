package dev.lookhin.flyminer;

import net.minecraft.client.Minecraft;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.tags.BlockTags;
import net.minecraft.tags.FluidTags;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.level.ClipContext;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.FallingBlock;
import net.minecraft.world.phys.HitResult;
import net.minecraft.world.phys.Vec3;
import java.util.*;

/** Emergency-only movement. Water is allowed here, never in the mining planner.
 * Searches loaded terrain in bounded slices, then rechecks every cut and movement tick. */
final class BreathEscape {
    private record Node(BlockPos pos,double cost) {}
    private static boolean active,water,canSwim;
    private static BlockPos origin,destination,cut;
    private static int age,cutAge,settle;
    private static double airBudget;
    private static final Map<BlockPos,Double> costs=new HashMap<>();
    private static final Map<BlockPos,BlockPos> parents=new HashMap<>();
    private static final PriorityQueue<Node> open=new PriorityQueue<>(Comparator.comparingDouble(Node::cost));
    private static BlockPos progress;
    private static final Map<BlockPos,Integer> visits=new HashMap<>();
    private static final Map<BlockPos,Boolean> searchMargins=new HashMap<>();

    static boolean active(){return active;}
    static void cancel(Minecraft mc){
        active=false;destination=null;cut=null;open.clear();costs.clear();parents.clear();
        mc.options.keyUp.setDown(false);mc.options.keyJump.setDown(false);mc.options.keyShift.setDown(false);
        if(mc.gameMode!=null)mc.gameMode.stopDestroyBlock();
    }
    static void begin(Minecraft mc){
        if(mc.player.isInLava() || mc.player.isPassenger())throw new IllegalStateException("Cannot use oxygen escape while in lava or riding.");
        if(!AirSupply.urgent(mc))throw new IllegalStateException("Oxygen escape is only available when air recovery is needed.");
        active=true;water=mc.player.isInWater();age=0;cutAge=0;settle=0;destination=null;cut=null;
        canSwim=MiningProfile.maySwim(water,MiningProfile.dwarf());
        origin=StandingGeometry.feet(mc);progress=origin;open.clear();costs.clear();parents.clear();
        searchMargins.clear();
        costs.put(origin,0.);open.add(new Node(origin,0.));
        var air=AirSupply.state(mc);airBudget=Math.max(1,((Number)air.get("air")).doubleValue()/20+((Boolean)air.get("divingGear")?((Number)air.get("tankAir")).doubleValue():0)-1);
    }
    private static boolean dangerous(Minecraft mc,BlockPos p){
        if(!mc.level.hasChunkAt(p) || mc.level.isOutsideBuildHeight(p))return true;
        var s=mc.level.getBlockState(p);
        return s.is(BlockTags.FIRE)||s.is(Blocks.MAGMA_BLOCK)||s.is(Blocks.CAMPFIRE)||s.is(Blocks.SOUL_CAMPFIRE)||
            s.is(Blocks.CACTUS)||s.is(Blocks.POWDER_SNOW)||s.is(Blocks.SWEET_BERRY_BUSH)||
            s.getBlock() instanceof FallingBlock||!s.getFluidState().isEmpty()&&!s.getFluidState().is(FluidTags.WATER);
    }
    private static boolean margin(Minecraft mc,BlockPos p){
        if(destination==null&&searchMargins.containsKey(p))return searchMargins.get(p);
        boolean value=marginChecked(mc,p);
        if(destination==null)searchMargins.put(p.immutable(),value);
        return value;
    }
    private static boolean marginChecked(Minecraft mc,BlockPos p){
        return LiquidSafety.cubeClear(p,q->mc.level.hasChunkAt(q) && !mc.level.isOutsideBuildHeight(q) &&
            (mc.level.getFluidState(q).isEmpty() || water&&mc.level.getFluidState(q).is(FluidTags.WATER)));
    }
    private static boolean clear(Minecraft mc,BlockPos p){return !dangerous(mc,p)&&Passages.clear(mc,p,mc.level.getBlockState(p));}
    private static boolean breakable(Minecraft mc,BlockPos p){
        if(dangerous(mc,p)||!margin(mc,p)||HomeSettings.protectedAt(mc,p))return false;
        var s=mc.level.getBlockState(p);
        if(s.hasBlockEntity() || s.getDestroySpeed(mc.level,p)<0 || !s.getFluidState().isEmpty())return false;
        // Do not release sand/gravel onto the player or a planned exit.
        if(dangerous(mc,p.above()) || mc.level.getBlockState(p.above()).getBlock() instanceof FallingBlock)return false;
        return true;
    }
    private static List<BlockPos> clearance(BlockPos a,BlockPos b){
        var result=new ArrayList<BlockPos>();
        if(b.getY()>a.getY())result.add(a.above(2));
        if(b.getY()<a.getY())result.add(b.above(2));
        result.add(b.above());result.add(b);
        return result.stream().distinct().toList();
    }
    private static double edge(Minecraft mc,BlockPos a,BlockPos b){
        if(!water&&(!mc.level.getFluidState(b).isEmpty()||!mc.level.getFluidState(b.above()).isEmpty()))return Double.POSITIVE_INFINITY;
        if(!margin(mc,b)||!margin(mc,b.above())||dangerous(mc,b)||dangerous(mc,b.above()))return Double.POSITIVE_INFINITY;
        boolean swimming=canSwim&&(mc.level.getFluidState(a).is(FluidTags.WATER)||mc.level.getFluidState(b).is(FluidTags.WATER));
        boolean floorSafe=!dangerous(mc,b.below())&&Passages.supports(mc,b.below(),mc.level.getBlockState(b.below()));
        if(!MiningProfile.supportedEscape(swimming,a.getX()==b.getX()&&a.getZ()==b.getZ(),floorSafe))return Double.POSITIVE_INFINITY;
        double cost=swimming?1.5:.65;
        for(var p:clearance(a,b))if(!clear(mc,p)){
            if(!breakable(mc,p))return Double.POSITIVE_INFINITY;
            cost+=ToolChoice.seconds(mc,p)+.2;
        }
        return cost+visits.getOrDefault(b,0)*2;
    }
    private static BlockPos first(BlockPos end){
        BlockPos p=end;
        for(int i=0;i<4096;i++) {BlockPos parent=parents.get(p);if(parent==null||parent.equals(origin))return p;p=parent;}
        throw new IllegalStateException("Invalid oxygen route.");
    }
    private static void search(Minecraft mc){
        long until=System.nanoTime()+4_000_000;
        while(!open.isEmpty()&&costs.size()<6000&&System.nanoTime()<until){
            Node n=open.poll();if(n.cost>costs.getOrDefault(n.pos,Double.POSITIVE_INFINITY))continue;
            var p=n.pos;
            if(!p.equals(origin) && AirSupply.fresh(mc,new Vec3(p.getX()+.5,p.getY()+mc.player.getEyeHeight(),p.getZ()+.5))) {destination=first(p);return;}
            if(p.getY()>progress.getY() || p.getY()==progress.getY()&&n.cost<costs.getOrDefault(progress,Double.POSITIVE_INFINITY))progress=p;
            var next=new ArrayList<BlockPos>();
            for(Direction d:Direction.Plane.HORIZONTAL)for(int y=1;y>=-1;y--)next.add(p.relative(d).offset(0,y,0));
            if(canSwim){next.add(p.above());next.add(p.below());}
            for(var q:next){
                if(Math.abs(q.getX()-origin.getX())>12||Math.abs(q.getZ()-origin.getZ())>12||q.getY()<origin.getY()-4||q.getY()>origin.getY()+24)continue;
                double candidate=n.cost+edge(mc,p,q);
                if(candidate>airBudget)continue;
                if(candidate<costs.getOrDefault(q,Double.POSITIVE_INFINITY)){
                    costs.put(q,candidate);parents.put(q,p);open.add(new Node(q,candidate));
                }
            }
        }
        if(open.isEmpty()||costs.size()>=6000){
            if(!progress.equals(origin))destination=first(progress);
            else throw new IllegalStateException("No checked oxygen exit: protected blocks, lava, unsupported floor or unloaded terrain. Manual rescue needed.");
        }
    }
    static String update(Minecraft mc){
        age++;
        if(AirSupply.fresh(mc,mc.player.getEyePosition())){
            mc.options.keyUp.setDown(false);mc.options.keyJump.setDown(canSwim&&mc.player.isInWater());
            mc.options.keyShift.setDown(false);
            if(mc.player.getAirSupply()>=mc.player.getMaxAirSupply()*.95){visits.clear();return "Reached breathable air and refilled oxygen.";}
            return null;
        }
        if(destination==null){search(mc);return null;}
        if(!Double.isFinite(edge(mc,origin,destination)))throw new IllegalStateException("Oxygen escape terrain changed; replan.");
        for(var p:clearance(origin,destination))if(!clear(mc,p)){
            mc.options.keyUp.setDown(false);mc.options.keyJump.setDown(false);
            if(!p.equals(cut)){cut=p;cutAge=0;settle=3;ToolChoice.equip(mc,mc.level.getBlockState(p));}
            if(settle-->0)return null;
            if(++cutAge>400)throw new IllegalStateException("Emergency block did not break; server may protect it or held tool cannot finish in time.");
            Vec3 point=Vec3.atCenterOf(p);
            var hit=mc.level.clip(new ClipContext(mc.player.getEyePosition(),point,ClipContext.Block.OUTLINE,ClipContext.Fluid.NONE,mc.player));
            if(hit.getType()!=HitResult.Type.BLOCK||!hit.getBlockPos().equals(p)||mc.player.getEyePosition().distanceTo(hit.getLocation())>Math.min(4.5,mc.gameMode.getPickRange()))
                throw new IllegalStateException("Oxygen escape block is hidden or out of reach; replan.");
            mc.gameMode.continueDestroyBlock(p,hit.getDirection());mc.player.swing(InteractionHand.MAIN_HAND);return null;
        }
        if(cut!=null){mc.gameMode.stopDestroyBlock();cut=null;}
        double dx=destination.getX()+.5-mc.player.getX(), dz=destination.getZ()+.5-mc.player.getZ();
        double distance=Math.hypot(dx,dz),dy=destination.getY()-mc.player.getY();
        if(distance<.16&&Math.abs(dy)<.2&&(mc.player.onGround()||mc.player.isInWater())) {
            visits.merge(destination,1,Integer::sum);return "Advanced one checked oxygen escape step.";
        }
        if(age>480||mc.player.position().distanceTo(Vec3.atBottomCenterOf(origin))>3)
            throw new IllegalStateException("Oxygen escape stalled or left its checked corridor.");
        mc.player.setSprinting(false);
        mc.player.setYRot((float)Math.toDegrees(Math.atan2(-dx,dz)));mc.player.setXRot(0);
        mc.options.keyUp.setDown(distance>Math.max(.07,mc.player.getDeltaMovement().horizontalDistance()*2+.04));
        mc.options.keyJump.setDown(canSwim?dy>.05:dy>.2&&mc.player.onGround());
        mc.options.keyShift.setDown(canSwim&&dy<-.2);
        return null;
    }
}
