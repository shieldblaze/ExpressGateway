package com.shieldblaze.expressgateway.core.configuration.eventloop;

/**
 * <p> {@link #NIO} uses Java NIO </p>
 * <p> {@link #EPOLL} uses Linux Epoll Mechanism </p>
 */
public enum EventLoopType {
    NIO, EPOLL
}
