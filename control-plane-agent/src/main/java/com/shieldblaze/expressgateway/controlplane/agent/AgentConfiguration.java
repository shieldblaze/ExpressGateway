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
package com.shieldblaze.expressgateway.controlplane.agent;

import java.nio.file.Path;
import java.util.Objects;

/**
 * Configuration for the Control Plane Agent.
 *
 * @param controlPlaneAddress the CP server address
 * @param controlPlanePort    the CP server gRPC port
 * @param nodeId              unique identifier for this data plane node
 * @param clusterId           the cluster this node belongs to
 * @param environment         environment name (e.g. "production", "staging")
 * @param localAddress        the advertised address of this node (for CP registration)
 * @param buildVersion        the build version of this data plane instance
 * @param authToken           the authentication token for CP registration
 * @param lkgPath             path to the LKG config file
 * @param tlsEnabled          whether to use TLS for the gRPC connection
 * @param tlsCertPath         path to the client TLS certificate (null if TLS disabled)
 * @param tlsKeyPath          path to the client TLS private key (null if TLS disabled)
 * @param tlsTrustCertPath    path to the CA cert for verifying the CP server (null if TLS disabled)
 */
public record AgentConfiguration(
        String controlPlaneAddress,
        int controlPlanePort,
        String nodeId,
        String clusterId,
        String environment,
        String localAddress,
        String buildVersion,
        String authToken,
        Path lkgPath,
        boolean tlsEnabled,
        Path tlsCertPath,
        Path tlsKeyPath,
        Path tlsTrustCertPath
) {

    public AgentConfiguration {
        Objects.requireNonNull(controlPlaneAddress, "controlPlaneAddress");
        if (controlPlanePort < 1 || controlPlanePort > 65535) {
            throw new IllegalArgumentException("controlPlanePort must be in [1, 65535]");
        }
        Objects.requireNonNull(nodeId, "nodeId");
        Objects.requireNonNull(clusterId, "clusterId");
        Objects.requireNonNull(environment, "environment");
        Objects.requireNonNull(localAddress, "localAddress");
        Objects.requireNonNull(buildVersion, "buildVersion");
        Objects.requireNonNull(authToken, "authToken");
        Objects.requireNonNull(lkgPath, "lkgPath");
        if (tlsEnabled) {
            Objects.requireNonNull(tlsCertPath, "tlsCertPath required when TLS enabled");
            Objects.requireNonNull(tlsKeyPath, "tlsKeyPath required when TLS enabled");
            Objects.requireNonNull(tlsTrustCertPath, "tlsTrustCertPath required when TLS enabled");
        }
    }

    /**
     * Create a minimal configuration for plaintext (non-TLS) connections.
     */
    public static AgentConfiguration plaintext(String cpAddress, int cpPort,
                                               String nodeId, String clusterId,
                                               String environment, Path lkgPath) {
        return new AgentConfiguration(
                cpAddress, cpPort, nodeId, clusterId, environment,
                "", "1.0.0", "default", lkgPath,
                false, null, null, null
        );
    }
}
