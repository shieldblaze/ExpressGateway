package com.shieldblaze.expressgateway.core.configuration.eventloop;

import io.netty.util.internal.ObjectUtil;

public final class EventLoopConfigurationBuilder {
    private EventLoopType eventLoopType;
    private int parentWorkers;
    private int childWorkers;

    private EventLoopConfigurationBuilder() {
    }

    public static EventLoopConfigurationBuilder newBuilder() {
        return new EventLoopConfigurationBuilder();
    }

    public EventLoopConfigurationBuilder withEventLoopType(EventLoopType eventLoopType) {
        this.eventLoopType = eventLoopType;
        return this;
    }

    public EventLoopConfigurationBuilder withParentWorkers(int parentWorkers) {
        this.parentWorkers = parentWorkers;
        return this;
    }

    public EventLoopConfigurationBuilder withChildWorkers(int childWorkers) {
        this.childWorkers = childWorkers;
        return this;
    }

    public EventLoopConfiguration build() {
        EventLoopConfiguration eventLoopConfiguration = new EventLoopConfiguration();
        eventLoopConfiguration.setEventLoopType(ObjectUtil.checkNotNull(eventLoopType, "Event Loop Type"));
        eventLoopConfiguration.setParentWorkers(ObjectUtil.checkPositive(parentWorkers, "Parent Workers"));
        eventLoopConfiguration.setChildWorkers(ObjectUtil.checkPositive(childWorkers, "Child Workers"));
        return eventLoopConfiguration;
    }
}
