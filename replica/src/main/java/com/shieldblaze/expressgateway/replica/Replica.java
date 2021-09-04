/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.replica;

import com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil;
import io.netty.bootstrap.ServerBootstrap;
import io.netty.channel.ChannelFuture;
import io.netty.channel.ChannelInitializer;
import io.netty.channel.ChannelOption;
import io.netty.channel.ChannelPipeline;
import io.netty.channel.EventLoopGroup;
import io.netty.channel.nio.NioEventLoopGroup;
import io.netty.channel.socket.SocketChannel;
import io.netty.channel.socket.nio.NioServerSocketChannel;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.util.UUID;
import java.util.concurrent.TimeUnit;

/**
 * {@link Replica} handles replication operations.
 */
public final class Replica implements Runnable {

    private static final Logger logger = LogManager.getLogger(Replica.class);

    public static final EventLoopGroup EVENT_LOOP_GROUP = new NioEventLoopGroup(Runtime.getRuntime().availableProcessors());
    public static final UUID ID = UUID.randomUUID();
    public static final Replica INSTANCE = new Replica();

    @Override
    public void run() {
        ServerBootstrap serverBootstrap = new ServerBootstrap()
                .group(EVENT_LOOP_GROUP)
                .channel(NioServerSocketChannel.class)
                .childOption(ChannelOption.SO_KEEPALIVE, true)
                .childHandler(new ChannelInitializer<SocketChannel>() {
                    @Override
                    protected void initChannel(SocketChannel ch) {
                        ChannelPipeline pipeline = ch.pipeline();

                        pipeline.addLast(HeartbeatHandler.INSTANCE);
                        pipeline.addLast(ReplicaDecoder.INSTANCE);
                        pipeline.addLast(ReplicaHandler.INSTANCE);
                    }
                });

        try {
            String address = SystemPropertyUtil.getPropertyOrEnv("replica-address", "0.0.0.0");
            int port = SystemPropertyUtil.getPropertyOrEnvInt("replica-port", "9110");

            ChannelFuture channelFuture = serverBootstrap.bind(address, port);
            channelFuture.await(30, TimeUnit.SECONDS);
            if (channelFuture.isSuccess()) {
                logger.info("Replica server started successfully");
            } else {
                logger.fatal("Replica server failed to start; Replica is unavailable", channelFuture.cause());
            }
        } catch (Exception ex) {
            logger.error("Caught error while starting Replica server", ex);
        }
    }

    private Replica() {
        // Prevent outside initialization
    }
}
