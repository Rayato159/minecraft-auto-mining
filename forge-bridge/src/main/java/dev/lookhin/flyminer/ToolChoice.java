package dev.lookhin.flyminer;

import net.minecraft.client.Minecraft;
import net.minecraft.core.BlockPos;
import net.minecraft.world.inventory.ClickType;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.item.enchantment.EnchantmentHelper;
import net.minecraft.world.item.enchantment.Enchantments;
import net.minecraft.world.level.block.state.BlockState;

/** Fastest usable tool for this block; bare hand is a real fallback, not a pickaxe requirement. */
final class ToolChoice {
    static boolean utilityTool(ItemStack stack) {
        return stack.is(net.minecraft.tags.ItemTags.SHOVELS) || stack.is(net.minecraft.tags.ItemTags.HOES) ||
            stack.is(net.minecraft.tags.ItemTags.AXES) ||
            stack.canPerformAction(net.minecraftforge.common.ToolActions.SHOVEL_DIG) ||
            stack.canPerformAction(net.minecraftforge.common.ToolActions.HOE_DIG) ||
            stack.canPerformAction(net.minecraftforge.common.ToolActions.AXE_DIG);
    }
    static double speed(ItemStack stack, BlockState block) {
        if (!stack.isEmpty() && stack.isDamageableItem() && stack.getDamageValue()>=stack.getMaxDamage()-5) return 0;
        double speed=stack.isEmpty()?1:stack.getDestroySpeed(block);
        int efficiency=EnchantmentHelper.getItemEnchantmentLevel(Enchantments.BLOCK_EFFICIENCY,stack);
        if(speed>1 && efficiency>0) speed+=efficiency*efficiency+1;
        // Minecraft divides destruction speed by 30 with the correct harvesting tool, otherwise 100.
        return speed/(!block.requiresCorrectToolForDrops() || stack.isCorrectToolForDrops(block)?30:100);
    }
    static int best(Minecraft mc, BlockState block) {
        return best(mc.player.getInventory().items, block, mc.player.getInventory().selected);
    }
    static boolean harvestable(Minecraft mc, BlockState block) {
        return !block.requiresCorrectToolForDrops() || mc.player.getInventory().items.stream()
            .anyMatch(s -> s.isCorrectToolForDrops(block) && speed(s,block)>0);
    }
    static int best(java.util.List<ItemStack> items, BlockState block, int selected) {
        boolean needsCorrect = block.requiresCorrectToolForDrops() && items.stream()
            .anyMatch(s -> s.isCorrectToolForDrops(block) && speed(s,block)>0);
        int best=-1; double score=needsCorrect ? -1 : speed(ItemStack.EMPTY,block);
        for(int i=0;i<items.size();i++) {
            ItemStack stack=items.get(i);
            if(needsCorrect && !stack.isCorrectToolForDrops(block))continue;
            double candidate=speed(stack,block);
            if(candidate<=0)continue;
            if(candidate>score || best<0 && candidate==score){best=i;score=candidate;}
        }
        // If hand is best, use an empty slot if available; otherwise the fastest held item.
        if(!needsCorrect && score<=speed(ItemStack.EMPTY,block))
            for(int i=0;i<items.size();i++) if(items.get(i).isEmpty()) return i;
        return best<0?selected:best;
    }
    static void equip(Minecraft mc, BlockState block) {
        int slot=best(mc,block);
        if(slot>=9) {
            if(mc.player.containerMenu!=mc.player.inventoryMenu)throw new IllegalStateException("Close inventory before changing digging tools.");
            mc.gameMode.handleInventoryMouseClick(mc.player.inventoryMenu.containerId,slot,8,ClickType.SWAP,mc.player);slot=8;
        }
        mc.player.getInventory().selected=slot;
    }
    static double seconds(Minecraft mc, BlockPos p) {
        BlockState block=mc.level.getBlockState(p);
        double hardness=block.getDestroySpeed(mc.level,p);
        if(hardness<0)return Double.POSITIVE_INFINITY;
        ItemStack tool=mc.player.getInventory().getItem(best(mc,block));
        double rate=speed(tool,block);
        if(net.minecraft.world.effect.MobEffectUtil.hasDigSpeed(mc.player))
            rate*=1+.2*(net.minecraft.world.effect.MobEffectUtil.getDigSpeedAmplification(mc.player)+1);
        if(mc.player.hasEffect(net.minecraft.world.effect.MobEffects.DIG_SLOWDOWN)) {
            int fatigue=mc.player.getEffect(net.minecraft.world.effect.MobEffects.DIG_SLOWDOWN).getAmplifier();
            rate*=switch(fatigue){case 0->.3;case 1->.09;case 2->.0027;default->.00081;};
        }
        if(mc.player.isEyeInFluid(net.minecraft.tags.FluidTags.WATER) && !EnchantmentHelper.hasAquaAffinity(mc.player))rate/=5;
        if(!mc.player.onGround())rate/=5;
        return Math.max(.1,hardness/Math.max(.001,rate)/20);
    }
}
