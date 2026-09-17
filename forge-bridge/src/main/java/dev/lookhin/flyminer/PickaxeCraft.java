package dev.lookhin.flyminer;

import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.screens.inventory.AbstractContainerScreen;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.tags.ItemTags;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.entity.player.StackedContents;
import net.minecraft.world.inventory.*;
import net.minecraft.world.item.*;
import net.minecraft.world.item.crafting.*;
import net.minecraft.world.level.ClipContext;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.phys.*;
import net.minecraftforge.common.ForgeHooks;
import java.util.*;
import java.util.function.Predicate;

/** Bounded client executor: normal recipe requests, block use and acknowledged menu clicks.
 * No inventory/world mutation, creative packets, wooden pickaxes, or item dropping. */
final class PickaxeCraft {
    private static boolean running;
    private static Item desired;
    private static BlockPos origin, station, placed;
    private static Block opening, placing;
    private static AbstractContainerMenu ownedMenu, pendingMenu;
    private static int age, wait, settle, revision, observed, stable, before, toolsBefore;
    private static Item pendingItem;
    private static boolean movingIntoInventory;
    private static Recipe<?> recipe;
    private static boolean furnaceClaimed;
    private static String lastReason="";

    static boolean active(){return running;}
    static boolean owned(Minecraft mc){
        if (!running || mc.player==null || !(mc.screen instanceof AbstractContainerScreen<?> screen)) return false;
        var menu=mc.player.containerMenu;
        if(screen.getMenu()!=menu) return false;
        return ownedMenu!=null ? menu==ownedMenu : opening==Blocks.CRAFTING_TABLE ? menu instanceof CraftingMenu : opening==Blocks.FURNACE && menu instanceof FurnaceMenu;
    }
    static int count(Minecraft mc, Predicate<ItemStack> filter){return mc.player.getInventory().items.stream().filter(filter).mapToInt(ItemStack::getCount).sum();}
    private static int count(Minecraft mc,Item item){return count(mc,s->s.is(item));}
    static boolean stone(ItemStack s){return s.is(ItemTags.STONE_TOOL_MATERIALS);}
    static boolean rawIron(ItemStack s){return s.is(Items.RAW_IRON)||s.is(Items.IRON_ORE)||s.is(Items.DEEPSLATE_IRON_ORE);}
    static boolean material(ItemStack s){return s.is(Items.DIAMOND)||s.is(Items.IRON_INGOT)||rawIron(s)||stone(s)||s.is(Items.STICK)||s.is(ItemTags.PLANKS)||s.is(ItemTags.LOGS)||s.is(Items.COAL)||s.is(Items.CHARCOAL)||s.is(Items.CRAFTING_TABLE)||s.is(Items.FURNACE);}
    private static String materialGroup(ItemStack s){
        if(rawIron(s))return "raw_iron";if(stone(s))return "stone";
        if(s.is(ItemTags.PLANKS))return "planks";if(s.is(ItemTags.LOGS))return "logs";
        if(s.is(Items.COAL)||s.is(Items.CHARCOAL))return "fuel";
        return net.minecraft.core.registries.BuiltInRegistries.ITEM.getKey(s.getItem()).toString();
    }
    static boolean keepMaterial(Minecraft mc,ItemStack stack){
        if(!material(stack))return false;
        String group=materialGroup(stack);
        int limit=CraftReserve.limit(group,needsPick(mc));
        int earlier=0;
        for(var item:mc.player.getInventory().items){
            if(item==stack)return earlier<limit;
            if(!item.isEmpty()&&materialGroup(item).equals(group))earlier+=item.getCount();
        }
        return false;
    }
    static boolean needsPick(Minecraft mc){return mc.player.getInventory().items.stream().noneMatch(s->ClientBridge.harvestsTarget(mc,s)&&(!s.isDamageableItem()||s.getMaxDamage()-s.getDamageValue()>32));}
    static boolean needsMaterial(Minecraft mc,ItemStack s){
        String group=materialGroup(s);
        if(CraftReserve.limit(group,false)>0)
            return count(mc,t->materialGroup(t).equals(group))<CraftReserve.limit(group,false);
        if(!needsPick(mc))return false;
        if(s.is(Items.DIAMOND))return count(mc,Items.DIAMOND)<3;
        if(s.is(Items.IRON_INGOT))return count(mc,Items.DIAMOND)<3 && count(mc,Items.IRON_INGOT)<3;
        if(rawIron(s))return count(mc,Items.DIAMOND)<3 && count(mc,Items.IRON_INGOT)+count(mc,PickaxeCraft::rawIron)<3;
        if(stone(s))return count(mc,PickaxeCraft::stone)<11;
        if(s.is(Items.STICK))return count(mc,Items.STICK)<2;
        if(s.is(ItemTags.PLANKS))return count(mc,t->t.is(ItemTags.PLANKS))<8;
        if(s.is(ItemTags.LOGS))return count(mc,t->t.is(ItemTags.LOGS))<2 && count(mc,t->t.is(ItemTags.PLANKS))<8;
        if(s.is(Items.COAL)||s.is(Items.CHARCOAL))return count(mc,t->t.is(Items.COAL)||t.is(Items.CHARCOAL))<1;
        return (s.is(Items.CRAFTING_TABLE)||s.is(Items.FURNACE)) && count(mc,s.getItem())==0;
    }
    private static BlockPos nearby(Minecraft mc,Block block){
        return StationSearch.nearest(
            BlockPos.betweenClosedStream(mc.player.blockPosition().offset(-3,-1,-3),mc.player.blockPosition().offset(3,2,3)),
            p->mc.level.hasChunkAt(p)&&mc.level.getBlockState(p).is(block)&&hit(mc,p)!=null,
            p->mc.player.getEyePosition().distanceToSqr(Vec3.atCenterOf(p)));
    }
    private static Item select(Minecraft mc){
        int planks=count(mc,s->s.is(ItemTags.PLANKS))+4*count(mc,s->s.is(ItemTags.LOGS));
        var stock=new CraftPlan.Stock(count(mc,Items.DIAMOND),count(mc,Items.IRON_INGOT),count(mc,PickaxeCraft::rawIron),count(mc,PickaxeCraft::stone),planks,count(mc,Items.STICK),
            count(mc,Items.CRAFTING_TABLE)>0||nearby(mc,Blocks.CRAFTING_TABLE)!=null,
            count(mc,Items.FURNACE)>0||nearby(mc,Blocks.FURNACE)!=null,
            count(mc,s->s.is(Items.COAL)||s.is(Items.CHARCOAL))>0,
            ClientBridge.harvestsTarget(mc,new ItemStack(Items.DIAMOND_PICKAXE)),ClientBridge.harvestsTarget(mc,new ItemStack(Items.IRON_PICKAXE)),ClientBridge.harvestsTarget(mc,new ItemStack(Items.STONE_PICKAXE)));
        lastReason=planks<(stock.table()?0:4)+(stock.sticks()>=2?0:2)?"Need logs/planks for a crafting table and two sticks.":
            "Need 3 diamonds, 3 iron ingots (or smeltable iron + furnace/fuel), or 3 stone tool materials. Wooden pickaxes are disabled.";
        return switch(CraftPlan.select(stock)){case DIAMOND->Items.DIAMOND_PICKAXE;case IRON->Items.IRON_PICKAXE;case STONE->Items.STONE_PICKAXE;case UNAVAILABLE->null;};
    }
    static Map<String,Object> preview(Minecraft mc){
        Item item=select(mc);
        return Map.of("possible",item!=null,"tier",item==null?"":net.minecraft.core.registries.BuiltInRegistries.ITEM.getKey(item).toString(),"reason",item==null?lastReason:"");
    }
    static void begin(Minecraft mc){
        if(!mc.player.onGround()||mc.player.isInWater()||mc.player.isInLava())throw new IllegalStateException("Crafting needs dry supported ground.");
        if(mc.player.getInventory().items.stream().filter(ItemStack::isEmpty).count()<2)throw new IllegalStateException("Crafting needs two empty inventory slots.");
        if(!mc.player.containerMenu.getCarried().isEmpty())throw new IllegalStateException("Clear cursor before crafting.");
        desired=select(mc);if(desired==null)throw new IllegalStateException(lastReason);
        toolsBefore=count(mc,s->s.is(desired)&&ClientBridge.harvestsTarget(mc,s)&&(!s.isDamageableItem()||s.getMaxDamage()-s.getDamageValue()>32));
        running=true;origin=mc.player.blockPosition();age=0;wait=0;settle=0;recipe=null;pendingItem=null;
        opening=null;ownedMenu=null;pendingMenu=null;station=null;placed=null;placing=null;furnaceClaimed=false;
    }
    static void cancel(Minecraft mc){
        boolean close=running && mc.player!=null && mc.player.containerMenu.getCarried().isEmpty() && (owned(mc)||mc.player.containerMenu==mc.player.inventoryMenu);
        running=false;ownedMenu=null;opening=null;pendingItem=null;pendingMenu=null;recipe=null;placing=null;placed=null;
        if(close)mc.player.closeContainer();
    }
    private static void close(Minecraft mc){
        if(ownedMenu!=null){if(!mc.player.containerMenu.getCarried().isEmpty())throw new IllegalStateException("Cursor is occupied; crafting paused.");mc.player.closeContainer();}
        ownedMenu=null;opening=null;station=null;settle=4;
    }
    private static BlockHitResult hit(Minecraft mc,BlockPos pos){
        if(mc.player.getEyePosition().distanceTo(Vec3.atCenterOf(pos))>Math.min(4.5,mc.gameMode.getPickRange()))return null;
        var hit=mc.level.clip(new ClipContext(mc.player.getEyePosition(),Vec3.atCenterOf(pos),ClipContext.Block.OUTLINE,ClipContext.Fluid.NONE,mc.player));
        return hit.getType()==HitResult.Type.BLOCK&&hit.getBlockPos().equals(pos)?hit:null;
    }
    private static void aim(Minecraft mc,Vec3 point){var d=point.subtract(mc.player.getEyePosition());mc.player.setYRot((float)(Math.toDegrees(Math.atan2(d.z,d.x))-90));mc.player.setXRot((float)-Math.toDegrees(Math.atan2(d.y,Math.hypot(d.x,d.z))));}
    private static void equip(Minecraft mc,Item item){
        var inv=mc.player.getInventory();int slot=-1;
        for(int i=0;i<inv.items.size();i++)if(inv.items.get(i).is(item)){slot=i;break;}
        if(slot<0)throw new IllegalStateException("Missing workstation item.");
        if(slot>=9){mc.gameMode.handleInventoryMouseClick(mc.player.inventoryMenu.containerId,slot,8,ClickType.SWAP,mc.player);slot=8;}
        inv.selected=slot;
    }
    private static void place(Minecraft mc,Block block){
        if(mc.player.containerMenu!=mc.player.inventoryMenu){close(mc);return;}
        var feet=mc.player.blockPosition();
        for(BlockPos p:BlockPos.betweenClosed(feet.offset(-2,0,-2),feet.offset(2,0,2))){
            if(p.getX()==feet.getX()&&p.getZ()==feet.getZ()||!mc.level.hasChunkAt(p)||!mc.level.getBlockState(p).isAir()||!mc.level.getBlockState(p.above()).isAir()||!mc.level.getFluidState(p).isEmpty())continue;
            if(!mc.level.getBlockState(p.below()).isFaceSturdy(mc.level,p.below(),Direction.UP))continue;
            // Leave narrow tunnels/doorways alone: only place in an open side area.
            int open=0;for(Direction d:Direction.Plane.HORIZONTAL)if(mc.level.getBlockState(p.relative(d)).isAir())open++;
            if(open<3 || p.equals(HomeSettings.get(mc).home==null?feet:HomeSettings.get(mc).home.pos()) || p.equals(HomeSettings.get(mc).mine==null?feet:HomeSettings.get(mc).mine.pos()))continue;
            Vec3 top=Vec3.atBottomCenterOf(p);if(mc.player.getEyePosition().distanceTo(top)>Math.min(4.5,mc.gameMode.getPickRange()))continue;
            var ray=mc.level.clip(new ClipContext(mc.player.getEyePosition(),top.add(0,-.01,0),ClipContext.Block.OUTLINE,ClipContext.Fluid.NONE,mc.player));
            if(!ray.getBlockPos().equals(p.below())||ray.getDirection()!=Direction.UP)continue;
            equip(mc,block.asItem());aim(mc,top);
            var result=mc.gameMode.useItemOn(mc.player,InteractionHand.MAIN_HAND,new BlockHitResult(top,Direction.UP,p.below(),false));
            if(!result.consumesAction())throw new IllegalStateException("Server refused workstation placement.");
            placed=p.immutable();placing=block;wait=0;return;
        }
        throw new IllegalStateException("No free supported side area to place workstation. Make room or place a table/furnace nearby.");
    }
    private static boolean open(Minecraft mc,Block block){
        if(ownedMenu!=null && (block==Blocks.CRAFTING_TABLE && ownedMenu instanceof CraftingMenu || block==Blocks.FURNACE && ownedMenu instanceof FurnaceMenu))return true;
        if(ownedMenu!=null){close(mc);return false;}
        BlockPos p=nearby(mc,block);
        if(p==null){if(count(mc,block.asItem())>0)place(mc,block);else craft(mc,block.asItem());return false;}
        var ray=hit(mc,p);if(ray==null)throw new IllegalStateException("Workstation is obstructed.");
        opening=block;station=p;wait=0;aim(mc,ray.getLocation());mc.player.setShiftKeyDown(false);
        if(!mc.gameMode.useItemOn(mc.player,InteractionHand.MAIN_HAND,ray).consumesAction())throw new IllegalStateException("Workstation did not open.");
        return false;
    }
    private static Recipe<?> recipe(Minecraft mc,Predicate<ItemStack> output,int grid){
        StackedContents stock=new StackedContents();mc.player.getInventory().fillStackedContents(stock);
        return mc.level.getRecipeManager().getAllRecipesFor(RecipeType.CRAFTING).stream()
            .filter(r->r.canCraftInDimensions(grid,grid)&&output.test(r.getResultItem(mc.level.registryAccess()))&&stock.canCraft(r,null))
            .sorted(Comparator.comparing(r->r.getId().toString())).findFirst().orElse(null);
    }
    private static void craft(Minecraft mc,Item item){craft(mc,s->s.is(item));}
    private static void craft(Minecraft mc,Predicate<ItemStack> output){
        int grid=mc.player.containerMenu instanceof CraftingMenu?3:2;
        var r=recipe(mc,output,grid);
        if(r==null)throw new IllegalStateException("No craftable server recipe for required material; check ingredients/recipe unlocks.");
        recipe=r;wait=0;mc.gameMode.handlePlaceRecipe(mc.player.containerMenu.containerId,r,false);
    }
    private static void click(Minecraft mc,int slot,boolean intoInventory){
        var menu=mc.player.containerMenu;var stack=menu.getSlot(slot).getItem();
        if(stack.isEmpty())throw new IllegalStateException("Crafting slot changed.");
        pendingMenu=menu;pendingItem=stack.getItem();before=count(mc,pendingItem);movingIntoInventory=intoInventory;
        revision=menu.getStateId();observed=revision;stable=0;wait=0;
        if(mc.getConnection()==null)throw new IllegalStateException("Disconnected while crafting.");
        mc.getConnection().send(new net.minecraft.network.protocol.game.ServerboundContainerClickPacket(menu.containerId,revision,slot,0,ClickType.QUICK_MOVE,ItemStack.EMPTY,new it.unimi.dsi.fastutil.ints.Int2ObjectOpenHashMap<>()));
    }
    private static int inventorySlot(Minecraft mc,Predicate<ItemStack> filter){
        var menu=mc.player.containerMenu;
        for(int i=0;i<menu.slots.size();i++){var s=menu.getSlot(i);if(s.container==mc.player.getInventory()&&!s.getItem().isEmpty()&&filter.test(s.getItem()))return i;}
        return -1;
    }
    private static void smelt(Minecraft mc){
        if(!open(mc,Blocks.FURNACE))return;
        var menu=(FurnaceMenu)ownedMenu;
        if(!furnaceClaimed){
            if(!menu.getSlot(0).getItem().isEmpty()||!menu.getSlot(1).getItem().isEmpty()||!menu.getSlot(2).getItem().isEmpty()||menu.isLit())throw new IllegalStateException("Furnace is already in use; refusing to alter another smelting job.");
            furnaceClaimed=true;
        }
        if(!menu.getSlot(2).getItem().isEmpty()){
            if(!menu.getSlot(2).getItem().is(Items.IRON_INGOT))throw new IllegalStateException("Unexpected furnace output.");
            click(mc,2,true);return;
        }
        if(count(mc,Items.IRON_INGOT)>=3){
            if(!menu.getSlot(0).getItem().isEmpty()){click(mc,0,true);return;}
            if(!menu.getSlot(1).getItem().isEmpty()){click(mc,1,true);return;}
            close(mc);return;
        }
        if(menu.getSlot(0).getItem().isEmpty()){
            int slot=inventorySlot(mc,PickaxeCraft::rawIron);if(slot<0)throw new IllegalStateException("Iron feedstock ran out.");
            var stack=menu.getSlot(slot).getItem();
            boolean valid=mc.level.getRecipeManager().getAllRecipesFor(RecipeType.SMELTING).stream().anyMatch(r->r.getIngredients().get(0).test(stack)&&r.getResultItem(mc.level.registryAccess()).is(Items.IRON_INGOT));
            if(!valid)throw new IllegalStateException("Server recipe does not smelt this ore into iron.");
            click(mc,slot,false);return;
        }
        if(!menu.isLit()&&menu.getSlot(1).getItem().isEmpty()){
            int slot=inventorySlot(mc,s->s.is(Items.COAL)||s.is(Items.CHARCOAL));
            if(slot<0)slot=inventorySlot(mc,s->s.is(ItemTags.PLANKS)&&ForgeHooks.getBurnTime(s,RecipeType.SMELTING)>0);
            if(slot<0)throw new IllegalStateException("No furnace fuel left.");
            click(mc,slot,false);
        }
    }
    /** null while busy, a success message only after the resulting tool is in player inventory. */
    static String update(Minecraft mc){
        if(++age>3400)throw new IllegalStateException("Crafting/smelting timed out; materials remain in inventory or workstation.");
        if(mc.player.position().distanceTo(Vec3.atBottomCenterOf(origin))>1.5)throw new IllegalStateException("Player moved while crafting.");
        if(!mc.player.containerMenu.getCarried().isEmpty())throw new IllegalStateException("Cursor is occupied; stopped without dropping items.");
        if(settle-- >0)return null;
        if(pendingItem!=null){
            if(mc.player.containerMenu!=pendingMenu)throw new IllegalStateException("Menu changed during an unconfirmed transfer.");
            if(pendingMenu.getStateId()!=observed){observed=pendingMenu.getStateId();stable=0;}else stable++;
            if(++wait>80)throw new IllegalStateException("No confirmed server reply for crafting transfer; refusing to repeat click.");
            int delta=count(mc,pendingItem)-before;
            if(pendingMenu.getStateId()==revision||stable<2||wait<4)return null;
            if(movingIntoInventory?delta<=0:delta>=0)throw new IllegalStateException("Server did not confirm material transfer.");
            pendingItem=null;pendingMenu=null;settle=2;return null;
        }
        if(placing!=null){
            if(mc.level.getBlockState(placed).is(placing)){placing=null;placed=null;settle=3;return null;}
            if(++wait>60)throw new IllegalStateException("Workstation placement not confirmed.");return null;
        }
        if(opening!=null&&ownedMenu==null){
            if(owned(mc)){ownedMenu=mc.player.containerMenu;settle=5;return null;}
            if(++wait>60)throw new IllegalStateException("Workstation menu did not open.");return null;
        }
        if(ownedMenu!=null&&!owned(mc))throw new IllegalStateException("Crafting menu closed or replaced.");
        if(recipe!=null){
            ItemStack expected=recipe.getResultItem(mc.level.registryAccess());var output=mc.player.containerMenu.getSlot(0).getItem();
            if(ItemStack.isSameItemSameTags(expected,output)&&output.getCount()>=expected.getCount()){recipe=null;click(mc,0,true);return null;}
            if(++wait>60)throw new IllegalStateException("Recipe placement failed; recipe may be locked or changed by the modpack.");return null;
        }
        if(count(mc,s->s.is(desired)&&ClientBridge.harvestsTarget(mc,s)&&(!s.isDamageableItem()||s.getMaxDamage()-s.getDamageValue()>32))>toolsBefore)return "Crafted and confirmed "+net.minecraft.core.registries.BuiltInRegistries.ITEM.getKey(desired)+".";
        // Finish retrieving our furnace input/fuel even after the third ingot arrives.
        if(ownedMenu instanceof FurnaceMenu){smelt(mc);return null;}
        int plankNeed=(count(mc,Items.STICK)>=2?0:2)+(count(mc,Items.CRAFTING_TABLE)>0||nearby(mc,Blocks.CRAFTING_TABLE)!=null?0:4);
        if(desired==Items.IRON_PICKAXE&&count(mc,Items.IRON_INGOT)<3&&count(mc,s->s.is(Items.COAL)||s.is(Items.CHARCOAL))==0)plankNeed+=2;
        if(count(mc,s->s.is(ItemTags.PLANKS))<plankNeed){craft(mc,s->s.is(ItemTags.PLANKS));return null;}
        if(count(mc,Items.STICK)<2){craft(mc,Items.STICK);return null;}
        if(desired==Items.IRON_PICKAXE&&count(mc,Items.IRON_INGOT)<3){
            if(count(mc,Items.FURNACE)==0&&nearby(mc,Blocks.FURNACE)==null){if(open(mc,Blocks.CRAFTING_TABLE))craft(mc,Items.FURNACE);return null;}
            smelt(mc);return null;
        }
        if(open(mc,Blocks.CRAFTING_TABLE))craft(mc,desired);
        return null;
    }
}
