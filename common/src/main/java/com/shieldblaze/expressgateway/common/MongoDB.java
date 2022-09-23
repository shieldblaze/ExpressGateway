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
package com.shieldblaze.expressgateway.common;

import com.mongodb.ConnectionString;
import com.mongodb.MongoClientSettings;
import com.mongodb.client.MongoClient;
import com.mongodb.client.MongoClients;
import com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil;
import dev.morphia.Datastore;
import dev.morphia.Morphia;
import dev.morphia.mapping.MapperOptions;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.conscrypt.Conscrypt;

import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLEngine;
import javax.net.ssl.TrustManager;
import javax.net.ssl.TrustManagerFactory;
import javax.net.ssl.X509ExtendedTrustManager;
import java.io.Closeable;
import java.net.Socket;
import java.security.KeyStore;
import java.security.SecureRandom;
import java.security.cert.CertificateException;
import java.security.cert.X509Certificate;

/**
 * This class holds {@link Datastore} for accessing
 * MongoDB database.
 */
public final class MongoDB implements Closeable {

    private static final Logger logger = LogManager.getLogger(MongoDB.class);
    private static final MongoDB INSTANCE = new MongoDB();

    private final MongoClient mongoClient;
    private final Datastore DATASTORE;

    private MongoDB() {
        SSLContext sslContext;
        try {
            TrustManagerFactory tmf = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
            tmf.init((KeyStore) null);

            TrustManager[] tm = tmf.getTrustManagers();
            assert tm.length == 1 : "TrustManagers must be 1 (one)";

            TrustManager[] delegateTrustManager = {new DelegatingTrustManager((X509ExtendedTrustManager) tm[0])};

            sslContext = SSLContext.getInstance("TLSv1.3", Conscrypt.newProvider());
            sslContext.init(null, delegateTrustManager, SecureRandom.getInstanceStrong());

            logger.info("Successfully initialized Conscrypt TLSv1.3 for MongoDB Connection");
        } catch (Exception ex) {
            logger.fatal("Failed to initialize Conscrypt TLSv1.3 for MongoDB Connection", ex);
            throw new RuntimeException(ex);
        }

        mongoClient = MongoClients.create(
                MongoClientSettings.builder()
                        .applyConnectionString(new ConnectionString(SystemPropertyUtil.getPropertyOrEnv("MONGO_CONNECTION_STRING")))
                        .applyToSslSettings(builder -> builder
                                .enabled(true)
                                .invalidHostNameAllowed(false)
                                .context(sslContext))
                        .build());

        DATASTORE = Morphia.createDatastore(mongoClient, SystemPropertyUtil.getPropertyOrEnv("MONGO_DATABASE"), MapperOptions.builder()
                .storeNulls(true)
                .storeEmpties(true)
                .ignoreFinals(false)
                .cacheClassLookups(true)
                .build());
    }

    public static Datastore getInstance() {
        return INSTANCE.DATASTORE;
    }

    public static MongoClient mongoClient() {
        return INSTANCE.mongoClient;
    }

    /**
     * Calls {@link #close()}
     */
    public static void shutdown() {
        INSTANCE.close();
    }

    /**
     * Close {@link MongoClient}
     */
    @Override
    public void close() {
        INSTANCE.mongoClient.close();
    }

    /**
     * Delegating {@link TrustManager}.
     *
     * @link <a href="https://github.com/google/conscrypt/issues/1033">See Bug Fix</a>
     */
    private static final class DelegatingTrustManager extends X509ExtendedTrustManager {

        private final X509ExtendedTrustManager delegate;

        private DelegatingTrustManager(X509ExtendedTrustManager delegate) {
            this.delegate = delegate;
        }

        @Override
        public void checkClientTrusted(X509Certificate[] chain, String authType, Socket socket) throws CertificateException {
            delegate.checkClientTrusted(chain, authType, socket);
        }

        public void checkServerTrusted(X509Certificate[] chain, String authType, Socket socket) throws CertificateException {
            // See: https://github.com/google/conscrypt/issues/1033#issuecomment-982701272
            if ("GENERIC".equals(authType)) {
                authType = "UNKNOWN";
            }

            delegate.checkServerTrusted(chain, authType, socket);
        }

        @Override
        public void checkClientTrusted(X509Certificate[] chain, String authType, SSLEngine engine) throws CertificateException {
            delegate.checkClientTrusted(chain, authType, engine);
        }

        @Override
        public void checkServerTrusted(X509Certificate[] chain, String authType, SSLEngine engine) throws CertificateException {
            delegate.checkServerTrusted(chain, authType, engine);
        }

        @Override
        public void checkClientTrusted(X509Certificate[] chain, String authType) throws CertificateException {
            delegate.checkClientTrusted(chain, authType);
        }

        @Override
        public void checkServerTrusted(X509Certificate[] chain, String authType) throws CertificateException {
            delegate.checkServerTrusted(chain, authType);
        }

        @Override
        public X509Certificate[] getAcceptedIssuers() {
            return delegate.getAcceptedIssuers();
        }
    }
}
