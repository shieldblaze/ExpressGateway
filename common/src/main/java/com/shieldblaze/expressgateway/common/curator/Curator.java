/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
package com.shieldblaze.expressgateway.common.curator;

import com.shieldblaze.expressgateway.common.ExpressGateway;
import org.apache.curator.RetryPolicy;
import org.apache.curator.framework.CuratorFramework;
import org.apache.curator.framework.CuratorFrameworkFactory;
import org.apache.curator.retry.RetryNTimes;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.apache.zookeeper.ClientCnxnSocketNetty;

import java.io.Closeable;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;

import static org.apache.zookeeper.client.ZKClientConfig.SECURE_CLIENT;
import static org.apache.zookeeper.client.ZKClientConfig.ZOOKEEPER_CLIENT_CNXN_SOCKET;

public final class Curator implements Closeable {

    private static final Logger logger = LogManager.getLogger(Curator.class);
    private static final Curator INSTANCE = new Curator();

    private CompletableFuture<Boolean> connectionFuture;
    private CuratorFramework curatorFramework;

    /**
     * Returns {@link CuratorFramework} instance
     */
    public static CuratorFramework getInstance() throws ExecutionException, InterruptedException {
        assert connectionFuture().get() : "Connection must be established before accessing CuratorFramework Instance";
        return INSTANCE.curatorFramework;
    }

    public static void init() {
        INSTANCE.connectionFuture = new CompletableFuture<>();
        if (ExpressGateway.getInstance().runningMode() == ExpressGateway.RunningMode.REPLICA) {

            // If ConnectionFuture is not 'null' then we have existing Curator instance running.
            // We will close the existing instance before we build fresh one.
            if (INSTANCE.connectionFuture != null) {
                logger.info("Closing existing Curator instance");
                INSTANCE.close();
            }

            // Use Netty client with TLS
            if (ExpressGateway.getInstance().zooKeeper().enableTLS()) {
                System.setProperty(SECURE_CLIENT, "true");

                // If KeyStore file is defined then we will load it for mTLS
                if (!ExpressGateway.getInstance().zooKeeper().keyStoreFile().isEmpty()) {
                    System.setProperty("zookeeper.ssl.keyStore.location", ExpressGateway.getInstance().zooKeeper().keyStoreFile());
                    System.setProperty("zookeeper.ssl.keyStore.password", new String(ExpressGateway.getInstance().zooKeeper().keyStorePasswordAsChars()));
                }

                System.setProperty("zookeeper.ssl.hostnameVerification", String.valueOf(ExpressGateway.getInstance().zooKeeper().hostnameVerification()));
                System.setProperty("zookeeper.ssl.trustStore.location", ExpressGateway.getInstance().zooKeeper().trustStoreFile());
                System.setProperty("zookeeper.ssl.trustStore.password", new String(ExpressGateway.getInstance().zooKeeper().trustStorePasswordAsChars()));
            }

            // Always use Netty transport
            System.setProperty(ZOOKEEPER_CLIENT_CNXN_SOCKET, ClientCnxnSocketNetty.class.getCanonicalName());

            RetryPolicy retryPolicy = new RetryNTimes(ExpressGateway.getInstance().zooKeeper().retryTimes(),
                    ExpressGateway.getInstance().zooKeeper().sleepMsBetweenRetries());

            CuratorFrameworkFactory.Builder builder = CuratorFrameworkFactory.builder()
                    .connectString(ExpressGateway.getInstance().zooKeeper().connectionString())
                    .retryPolicy(retryPolicy);

            INSTANCE.curatorFramework = builder.build();
            INSTANCE.curatorFramework.start();

            INSTANCE.connectionFuture.completeAsync(() -> {
                try {
                    INSTANCE.curatorFramework.blockUntilConnected(30, TimeUnit.SECONDS);

                    // When isConnected is true then connection has been established successfully
                    if (INSTANCE.curatorFramework.getZookeeperClient().isConnected()) {
                        logger.info("Started Apache Zookeeper Curator. Connected: {}", true);
                        return true;
                    } else {
                        logger.fatal("Failed to start Apache Zookeeper Curator");
                        return false;
                    }
                } catch (InterruptedException e) {
                    logger.error("ConnectionFuture sleep thread interrupted", e);
                    throw new RuntimeException(e);
                }
            });
        } else {
            INSTANCE.connectionFuture.complete(false);
            logger.info("Skipping ZooKeeper initialization because ZooKeeper was disabled");
        }
    }

    /**
     * This {@link CompletableFuture} returns {@link Boolean#TRUE} once
     * MongoDB connection has been successfully else returns {@link Boolean#FALSE}
     */
    public static CompletableFuture<Boolean> connectionFuture() {
        return INSTANCE.connectionFuture;
    }

    /**
     * Calls {@link #close()}
     */
    public static void shutdown() {
        INSTANCE.close();
    }

    @Override
    public void close() {
        if (curatorFramework != null) {
            curatorFramework.close();
        }
    }

    private Curator() {
        // Prevent outside initialization
    }
}
