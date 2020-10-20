/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.configuration.tls;

import com.shieldblaze.expressgateway.configuration.GenericConfiguration;

import java.util.Map;
import java.util.TreeMap;

/**
 * Configuration for TLS
 */
public final class TLSConfiguration extends GenericConfiguration {
    private final Map<String, CertificateKeyPair> certificateKeyPairMap = new TreeMap<>();
    private boolean forServer;

    /**
     * Get {@link CertificateKeyPair} for a Hostname
     * @param fqdn FQDN
     * @return {@link CertificateKeyPair} if found
     * @throws NullPointerException If Mapping is not found for a Hostname
     */
    public CertificateKeyPair getMapping(String fqdn) {
        try {
            CertificateKeyPair certificateKeyPair = certificateKeyPairMap.get(fqdn);

            // If `null` it means, Mapping was not found with FQDN then we'll try Wildcard.
            if (certificateKeyPair == null) {
                fqdn = "*" + fqdn.substring(fqdn.indexOf("."));
                certificateKeyPair = certificateKeyPairMap.get(fqdn);
                if (certificateKeyPair != null) {
                    return certificateKeyPair;
                }
            }
        } catch (Exception ex) {
            // Ignore
        }

        if (certificateKeyPairMap.containsKey("DEFAULT_HOST")) {
            return getDefault();
        }

        throw new NullPointerException("Mapping Not Found");
    }

    public CertificateKeyPair getDefault() {
        return certificateKeyPairMap.get("DEFAULT_HOST");
    }

    void setCertificateKeyPairMap(Map<String, CertificateKeyPair> certificateKeyPairMap) {
        this.certificateKeyPairMap.putAll(certificateKeyPairMap);
    }

    public boolean isForServer() {
        return forServer;
    }

    void setForServer(boolean forServer) {
        this.forServer = forServer;
    }
}
