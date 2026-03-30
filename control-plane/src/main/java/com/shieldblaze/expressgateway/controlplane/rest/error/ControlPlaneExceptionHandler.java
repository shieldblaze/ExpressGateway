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
package com.shieldblaze.expressgateway.controlplane.rest.error;

import com.shieldblaze.expressgateway.controlplane.kvstore.KVStoreException;
import com.shieldblaze.expressgateway.controlplane.rest.dto.ApiResponse;
import lombok.extern.log4j.Log4j2;
import org.springframework.http.HttpStatus;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.ExceptionHandler;
import org.springframework.web.bind.annotation.ResponseStatus;
import org.springframework.web.bind.annotation.RestControllerAdvice;

/**
 * Global exception handler for the control plane REST API.
 *
 * <p>Translates exceptions into structured {@link ApiResponse} error payloads
 * with appropriate HTTP status codes. Specific exception types are mapped to
 * semantic status codes; all unhandled exceptions fall through to 500.</p>
 */
@RestControllerAdvice(basePackages = "com.shieldblaze.expressgateway.controlplane.rest")
@Log4j2
public class ControlPlaneExceptionHandler {

    @ExceptionHandler(IllegalArgumentException.class)
    public ResponseEntity<ApiResponse<Void>> handleBadRequest(IllegalArgumentException e) {
        log.warn("Bad request: {}", e.getMessage());
        return ResponseEntity.badRequest().body(ApiResponse.error(e.getMessage()));
    }

    @ExceptionHandler(KVStoreException.class)
    public ResponseEntity<ApiResponse<Void>> handleKVStoreException(KVStoreException e) {
        log.error("KV store error [{}]: {}", e.code(), e.getMessage(), e);
        return switch (e.code()) {
            case KEY_NOT_FOUND -> ResponseEntity.status(HttpStatus.NOT_FOUND)
                    .body(ApiResponse.error(e.getMessage()));
            case VERSION_CONFLICT -> ResponseEntity.status(HttpStatus.CONFLICT)
                    .body(ApiResponse.error("Version conflict: " + e.getMessage()));
            case UNAUTHORIZED -> ResponseEntity.status(HttpStatus.FORBIDDEN)
                    .body(ApiResponse.error("Unauthorized: " + e.getMessage()));
            case OPERATION_TIMEOUT -> ResponseEntity.status(HttpStatus.GATEWAY_TIMEOUT)
                    .body(ApiResponse.error("Operation timed out: " + e.getMessage()));
            case CONNECTION_LOST, INTERNAL_ERROR -> ResponseEntity.status(HttpStatus.INTERNAL_SERVER_ERROR)
                    .body(ApiResponse.error("Internal error: " + e.getMessage()));
        };
    }

    @ExceptionHandler(UnsupportedOperationException.class)
    @ResponseStatus(HttpStatus.NOT_IMPLEMENTED)
    public ApiResponse<Void> handleUnsupported(UnsupportedOperationException e) {
        return ApiResponse.error(e.getMessage());
    }

    @ExceptionHandler(Exception.class)
    public ResponseEntity<ApiResponse<Void>> handleException(Exception e) {
        log.error("Unhandled exception in control plane REST API", e);
        return ResponseEntity.status(HttpStatus.INTERNAL_SERVER_ERROR)
                .body(ApiResponse.error(e.getMessage()));
    }
}
