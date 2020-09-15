package com.shieldblaze.expressgateway.core.configuration.buffer;

public class PooledByteBufAllocatorConfiguration {
    private boolean preferDirect;
    private int nHeapArena;
    private int nDirectArena;
    private int pageSize;
    private int maxOrder;
    private int smallCacheSize;
    private int normalCacheSize;
    private boolean useCacheForAllThreads;
    private int directMemoryCacheAlignment;

    public boolean isPreferDirect() {
        return preferDirect;
    }

    public void setPreferDirect(boolean preferDirect) {
        this.preferDirect = preferDirect;
    }

    public int getnHeapArena() {
        return nHeapArena;
    }

    public void setnHeapArena(int nHeapArena) {
        this.nHeapArena = nHeapArena;
    }

    public int getnDirectArena() {
        return nDirectArena;
    }

    public void setnDirectArena(int nDirectArena) {
        this.nDirectArena = nDirectArena;
    }

    public int getPageSize() {
        return pageSize;
    }

    public void setPageSize(int pageSize) {
        this.pageSize = pageSize;
    }

    public int getMaxOrder() {
        return maxOrder;
    }

    public void setMaxOrder(int maxOrder) {
        this.maxOrder = maxOrder;
    }

    public int getSmallCacheSize() {
        return smallCacheSize;
    }

    public void setSmallCacheSize(int smallCacheSize) {
        this.smallCacheSize = smallCacheSize;
    }

    public int getNormalCacheSize() {
        return normalCacheSize;
    }

    public void setNormalCacheSize(int normalCacheSize) {
        this.normalCacheSize = normalCacheSize;
    }

    public boolean isUseCacheForAllThreads() {
        return useCacheForAllThreads;
    }

    public void setUseCacheForAllThreads(boolean useCacheForAllThreads) {
        this.useCacheForAllThreads = useCacheForAllThreads;
    }

    public int getDirectMemoryCacheAlignment() {
        return directMemoryCacheAlignment;
    }

    public void setDirectMemoryCacheAlignment(int directMemoryCacheAlignment) {
        this.directMemoryCacheAlignment = directMemoryCacheAlignment;
    }
}
