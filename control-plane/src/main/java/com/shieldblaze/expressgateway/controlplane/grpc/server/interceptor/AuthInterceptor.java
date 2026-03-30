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
package com.shieldblaze.expressgateway.controlplane.grpc.server.interceptor;

import com.shieldblaze.expressgateway.controlplane.registry.NodeRegistry;
import io.grpc.Metadata;
import io.grpc.ServerCall;
import io.grpc.ServerCallHandler;
import io.grpc.ServerInterceptor;
import io.grpc.Status;
import lombok.extern.log4j.Log4j2;

import java.util.Objects;

/**
 * gRPC server interceptor that enforces session token authentication on all RPCs
 * except the initial {@code Register} call (which is how a node obtains its token).
 *
 * <p>The interceptor extracts the {@code x-session-token} header from the request
 * metadata. If the token is absent or empty for any RPC other than Register, the
 * call is closed immediately with {@link Status#UNAUTHENTICATED}.</p>
 *
 * <p>Full token-to-node validation is deferred to the service implementations
 * because the interceptor does not have access to the node_id from the request body
 * (gRPC interceptors only see metadata, not message contents). This interceptor
 * serves as a first-pass gate: reject calls that are clearly unauthenticated,
 * then let the service layer verify the token matches the claimed node.</p>
 */
@Log4j2
public final class AuthInterceptor implements ServerInterceptor {

    /**
     * Metadata key for the session token header.
     * Nodes must include this header on every RPC after registration.
     */
    static final Metadata.Key<String> SESSION_TOKEN_KEY =
            Metadata.Key.of("x-session-token", Metadata.ASCII_STRING_MARSHALLER);

    private final NodeRegistry registry;

    /**
     * @param registry the node registry for potential token validation; must not be null
     */
    public AuthInterceptor(NodeRegistry registry) {
        this.registry = Objects.requireNonNull(registry, "registry");
    }

    @Override
    public <ReqT, RespT> ServerCall.Listener<ReqT> interceptCall(
            ServerCall<ReqT, RespT> call,
            Metadata headers,
            ServerCallHandler<ReqT, RespT> next) {

        // Skip auth for the Register RPC -- it is the registration itself.
        // getBareMethodName() returns just the method name without the service prefix.
        String methodName = call.getMethodDescriptor().getBareMethodName();
        if ("Register".equals(methodName)) {
            return next.startCall(call, headers);
        }

        String token = headers.get(SESSION_TOKEN_KEY);
        if (token == null || token.isEmpty()) {
            log.warn("Rejecting unauthenticated call to {} from {}",
                    methodName, call.getAttributes());
            call.close(Status.UNAUTHENTICATED.withDescription("Missing session token"), new Metadata());
            // Return a no-op listener since the call is already closed.
            return new ServerCall.Listener<>() {};
        }

        // Token is present. Full validation (token <-> node_id mapping) happens
        // at the service level because the interceptor cannot inspect the request body.
        return next.startCall(call, headers);
    }
}
