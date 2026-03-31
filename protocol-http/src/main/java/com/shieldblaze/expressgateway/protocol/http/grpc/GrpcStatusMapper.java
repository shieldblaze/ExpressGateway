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
package com.shieldblaze.expressgateway.protocol.http.grpc;

/**
 * Maps HTTP status codes to gRPC status codes per the gRPC specification.
 *
 * <p>The mapping follows the canonical HTTP-to-gRPC status translation defined in
 * <a href="https://github.com/grpc/grpc/blob/master/doc/http-grpc-status-mapping.md">
 * gRPC HTTP Status Code Mapping</a>. This is critical for a proxy that terminates
 * or translates between HTTP and gRPC: when the proxy itself generates an HTTP error
 * response (e.g., 503 Service Unavailable because no backend is available), the gRPC
 * client expects a {@code grpc-status} trailer, not a raw HTTP status code.</p>
 *
 * <p>All methods are stateless and allocation-free on the hot path.</p>
 */
public final class GrpcStatusMapper {

    // gRPC status code constants per grpc/status.h
    /** The operation completed successfully. */
    public static final int OK = 0;
    /** The operation was cancelled (typically by the caller). */
    public static final int CANCELLED = 1;
    /** Unknown error. */
    public static final int UNKNOWN = 2;
    /** Client specified an invalid argument. */
    public static final int INVALID_ARGUMENT = 3;
    /** Deadline expired before operation could complete. */
    public static final int DEADLINE_EXCEEDED = 4;
    /** Some requested entity was not found. */
    public static final int NOT_FOUND = 5;
    /** Some entity that we attempted to create already exists. */
    public static final int ALREADY_EXISTS = 6;
    /** The caller does not have permission to execute the specified operation. */
    public static final int PERMISSION_DENIED = 7;
    /** Some resource has been exhausted (e.g., per-user quota, file system space). */
    public static final int RESOURCE_EXHAUSTED = 8;
    /** Operation was rejected because the system is not in a state required for execution. */
    public static final int FAILED_PRECONDITION = 9;
    /** The operation was aborted, typically due to a concurrency issue. */
    public static final int ABORTED = 10;
    /** Operation was attempted past the valid range. */
    public static final int OUT_OF_RANGE = 11;
    /** Operation is not implemented or not supported/enabled. */
    public static final int UNIMPLEMENTED = 12;
    /** Internal errors — invariant violation, unexpected null, etc. */
    public static final int INTERNAL = 13;
    /** The service is currently unavailable. */
    public static final int UNAVAILABLE = 14;
    /** Unrecoverable data loss or corruption. */
    public static final int DATA_LOSS = 15;
    /** The request does not have valid authentication credentials. */
    public static final int UNAUTHENTICATED = 16;

    /**
     * Canonical gRPC status code names indexed by code value.
     * Index == code, so lookup is O(1) with no branching.
     */
    private static final String[] STATUS_NAMES = {
            "OK",                  // 0
            "CANCELLED",           // 1
            "UNKNOWN",             // 2
            "INVALID_ARGUMENT",    // 3
            "DEADLINE_EXCEEDED",   // 4
            "NOT_FOUND",           // 5
            "ALREADY_EXISTS",      // 6
            "PERMISSION_DENIED",   // 7
            "RESOURCE_EXHAUSTED",  // 8
            "FAILED_PRECONDITION", // 9
            "ABORTED",             // 10
            "OUT_OF_RANGE",        // 11
            "UNIMPLEMENTED",       // 12
            "INTERNAL",            // 13
            "UNAVAILABLE",         // 14
            "DATA_LOSS",           // 15
            "UNAUTHENTICATED"      // 16
    };

    /**
     * Maps an HTTP status code to the corresponding gRPC status code per the
     * gRPC HTTP-to-gRPC status mapping specification.
     *
     * <p>Mapping table:</p>
     * <ul>
     *   <li>200 OK -> 0 (OK)</li>
     *   <li>400 Bad Request -> 3 (INVALID_ARGUMENT)</li>
     *   <li>401 Unauthorized -> 16 (UNAUTHENTICATED)</li>
     *   <li>403 Forbidden -> 7 (PERMISSION_DENIED)</li>
     *   <li>404 Not Found -> 12 (UNIMPLEMENTED)</li>
     *   <li>408 Request Timeout -> 4 (DEADLINE_EXCEEDED)</li>
     *   <li>409 Conflict -> 10 (ABORTED)</li>
     *   <li>429 Too Many Requests -> 8 (RESOURCE_EXHAUSTED)</li>
     *   <li>499 Client Closed Request -> 1 (CANCELLED)</li>
     *   <li>500 Internal Server Error -> 13 (INTERNAL)</li>
     *   <li>501 Not Implemented -> 12 (UNIMPLEMENTED)</li>
     *   <li>502 Bad Gateway -> 14 (UNAVAILABLE)</li>
     *   <li>503 Service Unavailable -> 14 (UNAVAILABLE)</li>
     *   <li>504 Gateway Timeout -> 4 (DEADLINE_EXCEEDED)</li>
     * </ul>
     *
     * <p>Any HTTP status code not explicitly listed maps to {@link #UNKNOWN} (2),
     * consistent with the gRPC spec's default behavior for unmapped codes.</p>
     *
     * @param httpStatus the HTTP response status code (e.g., 200, 503)
     * @return the corresponding gRPC status code (0-16)
     */
    public static int httpToGrpcStatus(int httpStatus) {
        // Switch is compiled to a tableswitch/lookupswitch by javac — no allocations,
        // branch-prediction-friendly for the common 200/502/503/504 cases.
        switch (httpStatus) {
            case 200:
                return OK;
            case 400:
                return INVALID_ARGUMENT;
            case 401:
                return UNAUTHENTICATED;
            case 403:
                return PERMISSION_DENIED;
            case 404:
                return UNIMPLEMENTED;
            case 408:
                return DEADLINE_EXCEEDED;
            case 409:
                return ABORTED;
            case 429:
                return RESOURCE_EXHAUSTED;
            case 499:
                return CANCELLED;
            case 500:
                return INTERNAL;
            case 501:
                return UNIMPLEMENTED;
            case 502:
                return UNAVAILABLE;
            case 503:
                return UNAVAILABLE;
            case 504:
                return DEADLINE_EXCEEDED;
            default:
                return UNKNOWN;
        }
    }

    /**
     * Returns the canonical name of a gRPC status code.
     *
     * @param code the gRPC status code (0-16)
     * @return the canonical name (e.g., "OK", "INTERNAL", "UNAVAILABLE"),
     *         or "UNKNOWN" if the code is out of the defined range
     */
    public static String grpcStatusName(int code) {
        if (code >= 0 && code < STATUS_NAMES.length) {
            return STATUS_NAMES[code];
        }
        return STATUS_NAMES[UNKNOWN];
    }

    private GrpcStatusMapper() {
        // Prevent outside initialization
    }
}
