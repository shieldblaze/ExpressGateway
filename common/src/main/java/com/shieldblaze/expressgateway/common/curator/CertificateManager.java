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

import com.shieldblaze.expressgateway.common.crypto.cryptostore.CryptoEntry;
import com.shieldblaze.expressgateway.common.crypto.cryptostore.CryptoStore;
import org.apache.curator.framework.recipes.cache.ChildData;
import org.apache.curator.framework.recipes.cache.CuratorCache;
import org.apache.curator.framework.recipes.cache.CuratorCacheListener;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.security.KeyStoreException;
import java.security.NoSuchAlgorithmException;
import java.security.UnrecoverableKeyException;
import java.security.cert.CertificateException;
import java.util.Optional;
import java.util.UUID;
import java.util.concurrent.CompletableFuture;

import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CLUSTER_ID;
import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CRYPTO_LOADBALANCER_PASSWORD;
import static java.lang.System.getProperty;
import static java.lang.System.setProperty;
import static java.util.Objects.requireNonNull;

public final class CertificateManager {

    private static final Logger logger = LogManager.getLogger(CertificateManager.class);
    private CuratorCache curatorCache;
    private final CompletableFuture<Boolean> INITIALIZED = new CompletableFuture<>();
    public static final CertificateManager INSTANCE = new CertificateManager();

    private CertificateManager() {
        if (Curator.ENABLED) {
            try {
                CuratorCacheListener listener = CuratorCacheListener.builder()
                        .forInitialized(() -> INITIALIZED.complete(true)) // Mark initialization successful
                        .build();

                curatorCache = CuratorCache.build(Curator.getInstance(), "/ExpressGateway");
                curatorCache.listenable().addListener(listener);
                curatorCache.start();
            } catch (Exception ex) {
                logger.fatal("Failed to initialize CertificateManager", ex);
            }
        } else {
            logger.info("CertificateManager is disabled because ZooKeeper is disabled");
            INITIALIZED.complete(false);
        }
    }

    public CompletableFuture<Boolean> isInitialized() {
        return INITIALIZED;
    }

    // ------------------- High-Level API -------------------

    public static CryptoEntry retrieveEntry(boolean server, String hostname) throws Exception {
        logger.info("Retrieving CryptoEntry from ZooKeeper");

        Optional<ChildData> childData = retrieve(server, hostname);
        if (childData.isEmpty()) {
            Exception exception = new IllegalStateException("Entry not found in ZooKeeper");
            logger.fatal("Failed to retrieve CryptoEntry from ZooKeeper", exception);
            throw exception;
        }

        logger.info("Processing CryptoEntry");
        try (ByteArrayInputStream inputStream = new ByteArrayInputStream(childData.get().getData())) {
            CryptoEntry cryptoEntry = CryptoStore.fetchPrivateKeyCertificateEntry(inputStream, getProperty(CRYPTO_LOADBALANCER_PASSWORD.name()).toCharArray(), hostname);
            logger.info("Successfully retrieved and processed CryptoEntry");
            return cryptoEntry;
        } catch (IOException | UnrecoverableKeyException | CertificateException | KeyStoreException | NoSuchAlgorithmException ex) {
            logger.fatal("Failed to retrieve CryptoEntry from ZooKeeper", ex);
            throw ex;
        }
    }

    public static void storeEntry(boolean server, String hostname, CryptoEntry cryptoEntry) throws Exception {
        logger.info("Storing CryptoEntry in ZooKeeper");

        try (ByteArrayOutputStream outputStream = new ByteArrayOutputStream()) {
            CryptoStore.storePrivateKeyAndCertificate(null, outputStream, getProperty(CRYPTO_LOADBALANCER_PASSWORD.name()).toCharArray(),
                    hostname, cryptoEntry.privateKey(), cryptoEntry.certificates());

            store(server, hostname, outputStream.toByteArray());
            logger.info("Successfully stored CryptoEntry in ZooKeeper");
        } catch (Exception ex) {
            logger.info("Failed to store CryptoEntry in ZooKeeper", ex);
            throw ex;
        }
    }

    // ------------------- Low-Level API -------------------

    public static Optional<ChildData> retrieve(boolean server, String hostname) {
        try {
            logger.info("Retrieving EncryptedCryptoEntry from ZooKeeper");
            ZNodePath zNodePath = of(server, hostname);
            return INSTANCE.curatorCache.get(zNodePath.path());
        } catch (Exception ex) {
            logger.fatal("Failed to retrieve EncryptedCryptoEntry from ZooKeeper", ex);
            throw ex;
        }
    }

    static void store(boolean server, String hostname, byte[] entry) throws Exception {
        logger.info("Storing EncryptedCryptoEntry in ZooKeeper");
        try {
            ZNodePath zNodePath = of(server, hostname);
            CuratorUtils.createNew(Curator.getInstance(), zNodePath, entry, true);
            logger.info("Successfully stored EncryptedCryptoEntry in ZooKeeper");
        } catch (Exception ex) {
            logger.fatal("Failed to store EncryptedCryptoEntry in ZooKeeper", ex);
            throw ex;
        }
    }

    public static void remove(boolean server, String hostname) throws Exception {
        logger.info("Removing EncryptedCryptoEntry from ZooKeeper");
        try {
            CuratorUtils.deleteData(Curator.getInstance(), of(server, hostname));
            logger.info("Successfully removed EncryptedCryptoEntry from ZooKeeper");
        } catch (Exception ex) {
            logger.fatal("Failed to remove EncryptedCryptoEntry from ZooKeeper", ex);
            throw ex;
        }
    }

    private static ZNodePath of(boolean server, String hostname) {
        Environment environment = Environment.detectEnv();

        String id = getProperty(CLUSTER_ID.name());
        if (environment == Environment.PRODUCTION) {
            requireNonNull(id, "ClusterID must be present");
        } else {
            // Generate Cluster ID in dev and qa environment
            if (id == null) {
                id = UUID.randomUUID().toString();
                setProperty(CLUSTER_ID.name(), id);
            }
        }

        return ZNodePath.create("ExpressGateway",
                environment,
                id,
                server ? "CertificateManagerServer" : "CertificateManagerClient",
                hostname);
    }
}
