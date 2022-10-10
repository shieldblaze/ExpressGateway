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
package com.shieldblaze.expressgateway.common.crypto;

import com.shieldblaze.expressgateway.common.curator.Curator;
import com.shieldblaze.expressgateway.common.curator.CuratorUtils;
import com.shieldblaze.expressgateway.common.curator.Environment;
import com.shieldblaze.expressgateway.common.curator.ZNodePath;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import static com.shieldblaze.expressgateway.common.SystemPropertiesKeys.CLUSTER_ID;
import static java.lang.System.getProperty;

public final class CertificateManager {

    private static final Logger logger = LogManager.getLogger(CertificateManager.class);
    public static final CertificateManager INSTANCE = new CertificateManager();

    public void storeInZooKeeper(boolean server, String hostname, byte[] encryptedData) throws Exception {
        logger.info("Storing EncryptedData in ZooKeeper");
        try {
            ZNodePath zNodePath = of(server, hostname);
            CuratorUtils.createNew(Curator.getInstance(), zNodePath, encryptedData);
            logger.info("Successfully stored EncryptedData in ZooKeeper");
        } catch (Exception ex) {
            logger.fatal("Failed to store EncryptedData in ZooKeeper", ex);
            throw ex;
        }
    }

    public void removeFromZooKeeper(boolean server, String hostname) throws Exception {
        try {
            logger.info("Removing EncryptedData from ZooKeeper");
            CuratorUtils.deleteData(Curator.getInstance(), of(server, hostname));
            logger.info("Successfully removed EncryptedData from ZooKeeper");
        } catch (Exception ex) {
            logger.fatal("Failed to remove EncryptedData from ZooKeeper", ex);
            throw ex;
        }
    }

    private ZNodePath of(boolean server, String hostname) {
        return ZNodePath.create("ExpressGateway",
                Environment.detectEnv(),
                getProperty(CLUSTER_ID.name()),
                server ? "TLSServerCertificateMapping" : "TLSClientCertificateMapping",
                hostname);
    }

    private CertificateManager() {
        // Prevent outside initialization
    }
}
