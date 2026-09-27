package dev.lookhin.flyminer;

import net.minecraft.core.BlockPos;
import net.minecraft.world.phys.shapes.Shapes;

public class CraftPlanTest {
    private static void check(CraftPlan.Tier expected,CraftPlan.Stock stock) {
        if(CraftPlan.select(stock)!=expected) throw new AssertionError(stock+" expected "+expected);
    }
    public static void main(String[] args) {
        if(MiningProfile.maySwim(true,true) || !MiningProfile.maySwim(true,false) || MiningProfile.maySwim(false,false))
            throw new AssertionError("Dwarf oxygen rescue must never rely on swimming");
        if(MiningProfile.supportedEscape(false,true,true) || MiningProfile.supportedEscape(false,false,false)
            || !MiningProfile.supportedEscape(false,false,true) || !MiningProfile.supportedEscape(true,true,false))
            throw new AssertionError("Non-swimmers need a supported horizontal/stair exit, never a water-column ascent");
        var dwarfBody=new net.minecraft.world.phys.AABB(.2,1,.2,.8,1.9,.8);
        if(!MiningProfile.coveredBody(dwarfBody,p->p.getX()==1)
            || MiningProfile.coveredBody(dwarfBody.move(.3,0,0),p->p.getX()==1)
            || MiningProfile.coveredBody(dwarfBody,p->true))
            throw new AssertionError("Check the whole actual body for exposure, including edges of a roof");
        var policyKeys=new java.util.HashSet<Long>();
        for(int flags=0;flags<16;flags++)policyKeys.add(MiningProfile.paletteKey(100,(flags&1)!=0,(flags&2)!=0,(flags&4)!=0,(flags&8)!=0));
        if(policyKeys.size()!=16 || policyKeys.contains(MiningProfile.paletteKey(101,false,false,false,false)))
            throw new AssertionError("Palette must distinguish shaded/sunlit air and roof cuts, even with identical block states");
        var liveSoul = new net.minecraft.world.phys.Vec3(67.5005682949782,111.875,-5.456484972078055);
        var soulFeet = new BlockPos(67,112,-6);
        if(!StandingGeometry.cell(liveSoul,true,true).equals(soulFeet)
            || StandingGeometry.cell(liveSoul,false,true).equals(soulFeet)
            || StandingGeometry.cell(liveSoul,true,false).equals(soulFeet))
            throw new AssertionError("Only grounded soul sand normalizes the live 111.875 standing height");
        if(!StandingGeometry.atHeight(liveSoul.y,StandingGeometry.floorY(soulFeet,true))
            || StandingGeometry.atHeight(liveSoul.y,StandingGeometry.floorY(soulFeet,false))
            || StandingGeometry.atHeight(111.5,StandingGeometry.floorY(soulFeet,true)))
            throw new AssertionError("Arrival must match the checked floor surface, never a generic larger tolerance");
        if(!StandingGeometry.cell(new net.minecraft.world.phys.Vec3(-.5,-.125,-.5),true,true).equals(new BlockPos(-1,0,-1)))
            throw new AssertionError("Fractional floor coordinates must work across zero");
        if(!PickaxeCraft.claimableFurnace(true,true,true)||PickaxeCraft.claimableFurnace(true,true,false)||PickaxeCraft.claimableFurnace(false,false,true))throw new AssertionError("Reuse only our empty furnace's residual heat, never another job");
        if(!AirSupply.low(300,300,true,true,true,100,900,120))throw new AssertionError("Full bubble bar must not hide an almost empty tank");
        if(AirSupply.low(300,300,true,true,true,800,900,120))throw new AssertionError("Charged breathing gear alone must not trigger low-air detection");
        if(!AirSupply.low(190,300,true,true,true,800,900,120))throw new AssertionError("Actual bubble depletion overrides a nominally charged tank");
        if(!AirSupply.low(300,300,true,false,false,0,0,120))throw new AssertionError("ThinAir exposure without protection must turn back immediately");
        if(!AirSupply.low(300,300,true,false,true,350,900,400))throw new AssertionError("Deep return distance increases the escape reserve");
        for(String id:new String[]{"minecraft:netherrack","minecraft:basalt","minecraft:smooth_basalt","minecraft:blackstone","minecraft:soul_soil","minecraft:soul_sand"})
            if(!NetherSafety.ROCKS.contains(id))throw new AssertionError("Nether terrain blocked: "+id);
        for(String id:new String[]{"minecraft:magma_block","minecraft:bedrock","minecraft:nether_bricks","minecraft:gilded_blackstone"})
            if(NetherSafety.ROCKS.contains(id))throw new AssertionError("Hazard/structure must not become terrain: "+id);
        if(!NetherSafety.oreId("minecraft:ancient_debris",false) || !NetherSafety.oreId("mod:crystal",true)
            || NetherSafety.oreId("mod:machine",false))throw new AssertionError("Explicit debris and tagged ores only");
        if(!NetherSafety.roof(BlockPos.ZERO,p->p.getY()==3) || NetherSafety.roof(BlockPos.ZERO,p->false))throw new AssertionError("Stable roof check");
        var retreatBody=new net.minecraft.world.phys.AABB(.2,1,.2,.8,2.8,.8);
        var rangedEnemy=new net.minecraft.world.phys.Vec3(10,1,0);
        if(!NetherSafety.farther(retreatBody,retreatBody.move(-1,0,0),rangedEnemy,true)
            || NetherSafety.farther(retreatBody,retreatBody.move(1,0,0),rangedEnemy,false)
            || NetherSafety.farther(retreatBody,retreatBody,rangedEnemy,true))throw new AssertionError("Retreat must increase enemy clearance");
        var bodyAtEdge = new net.minecraft.world.phys.AABB(.2,1,.2,.8,2.8,.8);
        java.util.function.Predicate<BlockPos> wet = p -> !p.equals(new BlockPos(2,1,0));
        if (!RecoverySafety.checked(bodyAtEdge,bodyAtEdge.move(-1,0,0),wet,b->true))
            throw new AssertionError("Must allow checked retreat away from water/lava/unknown buffer");
        if (RecoverySafety.checked(bodyAtEdge,bodyAtEdge.move(1,0,0),wet,b->true))
            throw new AssertionError("Must never approach fluid during retreat");
        if (RecoverySafety.checked(bodyAtEdge,bodyAtEdge.move(-1,0,0),wet,b->!(b.minX<0&&b.minX>-.5)))
            throw new AssertionError("A safe endpoint must not hide an unsupported gap in the sweep");
        if (RecoverySafety.checked(bodyAtEdge,bodyAtEdge.move(-1,-1,0),wet,b->true))
            throw new AssertionError("Emergency retreat must not drop from a ledge");
        if (RecoverySafety.checked(bodyAtEdge,bodyAtEdge.move(-1,0,0),p->wet.test(p)&&!p.equals(new BlockPos(-2,1,0)),b->true))
            throw new AssertionError("Must not retreat into a second liquid/unknown hazard");
        for(var heading:new int[][]{{1,0},{-1,0},{0,1},{0,-1}}) {
            var avoid=java.util.Set.of(new BlockPos(-1,1,0),new BlockPos(0,1,-1));
            var spots=TorchPlacement.candidates(BlockPos.ZERO,heading[0],heading[1],avoid);
            if(spots.isEmpty()||spots.get(0).face()==net.minecraft.core.Direction.UP)
                throw new AssertionError("Prefer rear wall torches");
            for(var spot:spots) {
                for(var p:java.util.List.of(spot.position(),spot.support()))
                    if(p.getX()*heading[0]+p.getZ()*heading[1]>=0||avoid.contains(p))
                        throw new AssertionError("Torch or support overlaps forward/planned excavation");
                if(!spot.support().relative(spot.face()).equals(spot.position()))
                    throw new AssertionError("Wrong torch support face");
            }
        }
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
        System.out.println("Recovery sweeps, rear torch geometry, session authorization, craft policies, BlockPos searches, passages, liquid buffers and reserve policies passed.");
        checkRealTools();
    }

    private static void checkRealTools() {
        net.minecraft.SharedConstants.tryDetectVersion();
        // Standalone JavaExec does not run Forge's networking bytecode transformers.
        // Supply the listener list normally generated for NetworkEvent by that transformer.
        try {
            var field=net.minecraftforge.eventbus.api.EventListenerHelper.class.getDeclaredField("listeners");
            field.setAccessible(true);
            var lists=field.get(null);
            seedListenerList(net.minecraftforge.network.NetworkEvent.class,lists);
            for(var event:net.minecraftforge.network.NetworkEvent.class.getDeclaredClasses()) {
                if(net.minecraftforge.eventbus.api.Event.class.isAssignableFrom(event))
                    seedListenerList(event,lists);
            }
        } catch(ReflectiveOperationException error) {throw new AssertionError(error);}
        net.minecraft.server.Bootstrap.bootStrap();
        // Normally run by the server's datapack reload, absent in standalone validation.
        try {
            var recalculate=net.minecraftforge.common.TierSortingRegistry.class.getDeclaredMethod("recalculateItemTiers");
            recalculate.setAccessible(true);recalculate.invoke(null);
        } catch(ReflectiveOperationException error) {throw new AssertionError(error);}
        var blocks=net.minecraft.core.registries.BuiltInRegistries.BLOCK;
        var dirt=net.minecraft.world.level.block.Blocks.DIRT;
        var soul=net.minecraft.world.level.block.Blocks.SOUL_SAND;
        var ore=net.minecraft.world.level.block.Blocks.ANCIENT_DEBRIS;
        var leaves=net.minecraft.world.level.block.Blocks.OAK_LEAVES;
        blocks.bindTags(java.util.Map.of(
            net.minecraft.tags.BlockTags.MINEABLE_WITH_SHOVEL,java.util.List.of(dirt.builtInRegistryHolder(),soul.builtInRegistryHolder()),
            net.minecraft.tags.BlockTags.MINEABLE_WITH_PICKAXE,java.util.List.of(ore.builtInRegistryHolder()),
            net.minecraft.tags.BlockTags.NEEDS_DIAMOND_TOOL,java.util.List.of(ore.builtInRegistryHolder()),
            net.minecraft.tags.BlockTags.MINEABLE_WITH_HOE,java.util.List.of(leaves.builtInRegistryHolder())));
        var pick=new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.DIAMOND_PICKAXE);
        var shovel=new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.IRON_SHOVEL);
        var hoe=new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.IRON_HOE);
        var gold=new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.GOLDEN_PICKAXE);
        var empty=net.minecraft.world.item.ItemStack.EMPTY;
        var inventory=java.util.List.of(pick,shovel,hoe,gold,empty);
        if(!ToolChoice.utilityTool(shovel) || !ToolChoice.utilityTool(hoe) ||
            !ToolChoice.utilityTool(new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.IRON_AXE)) ||
            ToolChoice.utilityTool(new net.minecraft.world.item.ItemStack(net.minecraft.world.item.Items.DIRT)))
            throw new AssertionError("Keep auxiliary digging tools when depositing ordinary cargo");
        if(ToolChoice.best(inventory,dirt.defaultBlockState(),0)!=1 || ToolChoice.best(inventory,soul.defaultBlockState(),0)!=1)
            throw new AssertionError("Dirt and soul sand must select a shovel, not the previous pickaxe");
        if(ToolChoice.best(inventory,leaves.defaultBlockState(),0)!=2)
            throw new AssertionError("Hoe-effective blocks must select a hoe");
        if(ToolChoice.best(inventory,ore.defaultBlockState(),1)!=0)
            throw new AssertionError("Faster gold pick must never outrank the diamond tool required to harvest debris");
        if(ToolChoice.best(java.util.List.of(pick,empty),dirt.defaultBlockState(),0)!=1)
            throw new AssertionError("No shovel permits empty-hand fallback on dirt");
        shovel.setDamageValue(shovel.getMaxDamage()-3);
        if(ToolChoice.best(inventory,soul.defaultBlockState(),0)==1)
            throw new AssertionError("Nearly broken shovel must not be selected");
        double top=soul.defaultBlockState().getCollisionShape(net.minecraft.world.level.EmptyBlockGetter.INSTANCE,BlockPos.ZERO)
            .max(net.minecraft.core.Direction.Axis.Y);
        if(Math.abs(top-.875)>1e-6)throw new AssertionError("Standing fixture disagrees with Minecraft's actual soul-sand collision shape");
        System.out.println("Actual Minecraft tool choices and soul-sand collision shape passed.");
    }

    private static net.minecraftforge.eventbus.ListenerList seedListenerList(Class<?> event,
            Object lists) throws ReflectiveOperationException {
        if(event==net.minecraftforge.eventbus.api.Event.class)
            return net.minecraftforge.eventbus.api.EventListenerHelper.getListenerList(event);
        var parent=seedListenerList(event.getSuperclass(),lists);
        // Forge 47.4.13 uses LockHelper/Supplier; newer eventbus versions use Cache/Function.
        for(var method:lists.getClass().getMethods()) {
            if(!method.getName().equals("computeIfAbsent") || method.getParameterCount()!=2)continue;
            Object factory;
            if(method.getParameterTypes()[1]==java.util.function.Supplier.class)
                factory=(java.util.function.Supplier<net.minecraftforge.eventbus.ListenerList>)() -> new net.minecraftforge.eventbus.ListenerList(parent);
            else if(method.getParameterTypes()[1]==java.util.function.Function.class)
                factory=(java.util.function.Function<Object,net.minecraftforge.eventbus.ListenerList>)ignored -> new net.minecraftforge.eventbus.ListenerList(parent);
            else continue;
            return (net.minecraftforge.eventbus.ListenerList)method.invoke(lists,event,factory);
        }
        throw new NoSuchMethodException("Unsupported standalone eventbus listener cache");
    }
}
