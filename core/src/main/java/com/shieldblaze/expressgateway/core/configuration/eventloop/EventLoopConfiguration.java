package com.shieldblaze.expressgateway.core.configuration.eventloop;

/**
 * Set {@code EventLoop} Configuration
 */
public class EventLoopConfiguration {
    private EventLoopType eventLoopType;
    private int parentWorkers;
    private int childWorkers;

    EventLoopConfiguration() {
        // Prevent outside initialization
    }

    public EventLoopType getEventLoopType() {
        return eventLoopType;
    }

    void setEventLoopType(EventLoopType eventLoopType) {
        this.eventLoopType = eventLoopType;
    }

    public int getParentWorkers() {
        return parentWorkers;
    }

    void setParentWorkers(int parentWorkers) {
        this.parentWorkers = parentWorkers;
    }

    public int getChildWorkers() {
        return childWorkers;
    }

    void setChildWorkers(int childWorkers) {
        this.childWorkers = childWorkers;
    }
}
