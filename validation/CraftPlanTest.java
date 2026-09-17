package dev.lookhin.flyminer;

import net.minecraft.core.BlockPos;
import net.minecraft.world.phys.shapes.Shapes;

public class CraftPlanTest {
    private static void check(CraftPlan.Tier expected,CraftPlan.Stock stock) {
        if(CraftPlan.select(stock)!=expected) throw new AssertionError(stock+" expected "+expected);
    }
    public static void main(String[] args) {
        SessionGuard session = new SessionGuard();
        String original = "world-a|player-a|minecraft:overworld";
        if(session.allows(original))throw new AssertionError("Control must start disabled");
        session.enable(original);
        if(!session.allows(original))throw new AssertionError("Explicitly enabled session rejected");
        for(String other:new String[]{"world-b|player-a|minecraft:overworld", "world-a|player-b|minecraft:overworld", "world-a|player-a|minecraft:the_nether"})
            if(session.allows(other))throw new AssertionError("Control leaked into another session");
        session.clear();
        if(session.allows(original))throw new AssertionError("Stopped session remained authorized");
        check(CraftPlan.Tier.DIAMOND,new CraftPlan.Stock(3,64,64,64,8,2,false,false,true,true,true,true));
        check(CraftPlan.Tier.IRON,new CraftPlan.Stock(2,3,0,3,6,0,false,false,false,true,true,true));
        check(CraftPlan.Tier.IRON,new CraftPlan.Stock(0,0,3,8,8,0,false,false,false,true,true,true));
        check(CraftPlan.Tier.STONE,new CraftPlan.Stock(0,0,3,3,6,0,false,false,false,true,true,true));
        check(CraftPlan.Tier.UNAVAILABLE,new CraftPlan.Stock(3,3,3,64,5,0,false,false,true,true,true,true));
        check(CraftPlan.Tier.UNAVAILABLE,new CraftPlan.Stock(0,0,0,0,64,64,true,true,true,true,true,true));
        check(CraftPlan.Tier.UNAVAILABLE,new CraftPlan.Stock(0,0,0,64,6,0,false,false,true,true,true,false));
        check(CraftPlan.Tier.IRON,new CraftPlan.Stock(0,2,1,0,0,2,true,true,true,true,true,false));
        check(CraftPlan.Tier.DIAMOND,new CraftPlan.Stock(3,0,0,0,0,2,true,false,false,true,true,false));
        BlockPos selected=StationSearch.nearest(BlockPos.betweenClosedStream(new BlockPos(-3,-1,-3),new BlockPos(3,2,3)),
            p->p.getY()==0 && p.getZ()==0 && (p.getX()==1 || p.getX()==2),p->p.distSqr(BlockPos.ZERO));
        if(!new BlockPos(1,0,0).equals(selected))throw new AssertionError("Mutable iteration corrupted nearest workstation: "+selected);
        if(StationSearch.nearest(BlockPos.betweenClosedStream(BlockPos.ZERO,new BlockPos(1,1,1)),p->false,p->0)!=null)
            throw new AssertionError("Absent workstation must stay absent");
        if(!Passages.stairTop(Shapes.or(Shapes.box(0,0,0,1,.5,1),Shapes.box(.5,.5,0,1,1,1))))throw new AssertionError("Stair top must support a centered player");
        if(Passages.stairTop(Shapes.box(0,0,0,1,.5,1))||Passages.stairTop(Shapes.box(.4,0,.4,.6,1.5,.6)))throw new AssertionError("Half slab/fence must not be treated as integer-height floor");
        if(!Passages.centerEmpty(Shapes.box(0,0,0,.1875,1,1))||Passages.centerEmpty(Shapes.block()))throw new AssertionError("Door center clearance is wrong");
        for(int x=-2;x<=2;x++)for(int y=-2;y<=2;y++)for(int z=-2;z<=2;z++) {
            BlockPos liquid=new BlockPos(x,y,z);
            if(LiquidSafety.cubeClear(BlockPos.ZERO,p->!p.equals(liquid)))throw new AssertionError("Missed liquid/unknown at "+liquid);
        }
        if(!LiquidSafety.cubeClear(BlockPos.ZERO,p->!p.equals(new BlockPos(3,0,0))))throw new AssertionError("Two intervening cells must be allowed");
        var body=new net.minecraft.world.phys.AABB(.65,1,.2,1.25,2.8,.8);
        if(LiquidSafety.boxClear(body,p->!p.equals(new BlockPos(3,1,0))))throw new AssertionError("Real body crossing a cell edge must keep its buffer");
        for(String group:new String[]{"stone","planks","logs","minecraft:stick","fuel"}) {
            if(CraftReserve.limit(group,false)!=64 || !CraftReserve.keep(group,false,0) || !CraftReserve.keep(group,false,63) || CraftReserve.keep(group,false,64))throw new AssertionError("Wrong retained/excess quota: "+group);
        }
        if(CraftReserve.keep("minecraft:diamond",false,0)||!CraftReserve.keep("minecraft:diamond",true,0))throw new AssertionError("Emergency metal reserves changed");
        System.out.println("Session authorization, craft policies, BlockPos searches, passage geometry, 127 liquid-buffer cases and 6 reserve policies passed.");
    }
}
