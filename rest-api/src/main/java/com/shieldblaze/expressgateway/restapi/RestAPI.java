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
package com.shieldblaze.expressgateway.restapi;

import com.shieldblaze.expressgateway.common.utils.NetworkInterfaceUtil;
import com.shieldblaze.expressgateway.common.utils.SelfSignedCertificate;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.bouncycastle.jce.provider.BouncyCastleProvider;
import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;
import org.springframework.boot.context.event.ApplicationFailedEvent;
import org.springframework.boot.context.event.ApplicationReadyEvent;
import org.springframework.boot.web.server.Ssl;
import org.springframework.context.ConfigurableApplicationContext;
import org.springframework.context.event.EventListener;

import java.io.File;
import java.io.FileOutputStream;
import java.security.KeyStore;
import java.security.Security;
import java.security.cert.Certificate;
import java.util.UUID;

@SpringBootApplication
public class RestAPI {

    private static final Logger logger = LogManager.getLogger(RestAPI.class);

    static final Ssl SSL = new Ssl();
    private static final String KEYSTORE_FILENAME = "keystore_" + System.nanoTime();
    private static ConfigurableApplicationContext ctx;

    static {
        // Initialize BouncyCastle first because we need it for TLS configuration
        Security.addProvider(new BouncyCastleProvider());
    }

    /**
     * Start the REST API Server using self-signed certificate.
     */
    public static void start() {
        configureTLS();
        ctx = SpringApplication.run(RestAPI.class);
    }

    /**
     * Shutdown REST-API Server
     */
    public static void stop() {
        if (ctx != null) {
            ctx.close();
        }
    }

    /**
     * Generate Self-signed certificate and keypair, add them to Keystore and load them
     */
    private static void configureTLS() {
        try {
            SelfSignedCertificate ssc = SelfSignedCertificate.generateNew(NetworkInterfaceUtil.getAllIps());

            FileOutputStream fos = new FileOutputStream(KEYSTORE_FILENAME);
            String uuid = UUID.randomUUID().toString();
            String alias = "ExpressGatewaySelfSignedKeystore";

            KeyStore keyStore = KeyStore.getInstance("BKS", "BC");
            keyStore.load(null, null);
            keyStore.setKeyEntry(alias, ssc.keyPair().getPrivate(), uuid.toCharArray(), new Certificate[]{ssc.x509Certificate()});
            keyStore.store(fos, uuid.toCharArray());
            fos.close();

            SSL.setKeyStore(KEYSTORE_FILENAME);
            SSL.setKeyStoreType("BKS");
            SSL.setKeyStoreProvider("BC");
            SSL.setKeyStorePassword(uuid);
            SSL.setKeyAlias(alias);
            SSL.setProtocol("TLSv1.3");
        } catch (Exception ex) {
            logger.error("Failed to configure TLS settings | Shutting down...", ex);
        }
    }

    /**
     * This event listener is used to keystore file once Spring Boot application
     * has succeeded or failed.
     */
    @EventListener({ApplicationReadyEvent.class, ApplicationFailedEvent.class})
    public void applicationEventCalled() {
        try {
            assert new File(KEYSTORE_FILENAME).delete();
        } catch (Exception ex) {
            logger.warn("Failed to delete Keystore file", ex);
        }
    }
}
