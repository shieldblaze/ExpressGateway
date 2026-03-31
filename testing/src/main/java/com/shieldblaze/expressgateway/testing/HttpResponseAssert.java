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
package com.shieldblaze.expressgateway.testing;

import java.net.http.HttpResponse;
import java.util.Objects;

/**
 * Fluent assertion helper for {@link HttpResponse} objects. Reduces test boilerplate
 * when verifying HTTP response status codes, headers, and body content.
 *
 * <p>Example:</p>
 * <pre>
 *   HttpResponseAssert.assertThat(response)
 *       .hasStatusCode(200)
 *       .hasHeader("Content-Type", "application/json")
 *       .bodyContains("Success");
 * </pre>
 */
public final class HttpResponseAssert {

    private final HttpResponse<String> response;

    private HttpResponseAssert(HttpResponse<String> response) {
        this.response = Objects.requireNonNull(response, "response must not be null");
    }

    /**
     * Start a fluent assertion chain for the given HTTP response.
     */
    public static HttpResponseAssert assertThat(HttpResponse<String> response) {
        return new HttpResponseAssert(response);
    }

    /**
     * Assert the response has the expected status code.
     */
    public HttpResponseAssert hasStatusCode(int expected) {
        int actual = response.statusCode();
        if (actual != expected) {
            throw new AssertionError("Expected status code " + expected + " but was " + actual +
                    ". Body: " + truncate(response.body(), 300));
        }
        return this;
    }

    /**
     * Assert the response status code is in the 2xx range.
     */
    public HttpResponseAssert isSuccessful() {
        int code = response.statusCode();
        if (code < 200 || code >= 300) {
            throw new AssertionError("Expected 2xx status code but was " + code +
                    ". Body: " + truncate(response.body(), 300));
        }
        return this;
    }

    /**
     * Assert the response has a header with the given name.
     */
    public HttpResponseAssert hasHeader(String name) {
        if (response.headers().firstValue(name).isEmpty()) {
            throw new AssertionError("Expected header '" + name + "' to be present but it was not. " +
                    "Headers: " + response.headers().map());
        }
        return this;
    }

    /**
     * Assert the response has a header with the given name and value.
     */
    public HttpResponseAssert hasHeader(String name, String expectedValue) {
        String actual = response.headers().firstValue(name).orElse(null);
        if (actual == null) {
            throw new AssertionError("Expected header '" + name + "' to be present but it was not");
        }
        if (!actual.equals(expectedValue)) {
            throw new AssertionError("Expected header '" + name + "' to be '" + expectedValue +
                    "' but was '" + actual + "'");
        }
        return this;
    }

    /**
     * Assert the response body is not null and not empty.
     */
    public HttpResponseAssert hasBody() {
        String body = response.body();
        if (body == null || body.isEmpty()) {
            throw new AssertionError("Expected response to have a body but it was null/empty");
        }
        return this;
    }

    /**
     * Assert the response body contains the given substring.
     */
    public HttpResponseAssert bodyContains(String expected) {
        String body = response.body();
        if (body == null || !body.contains(expected)) {
            throw new AssertionError("Expected body to contain '" + expected +
                    "' but body was: " + truncate(body, 300));
        }
        return this;
    }

    /**
     * Assert the response body equals the given string exactly.
     */
    public HttpResponseAssert bodyEquals(String expected) {
        String body = response.body();
        if (!Objects.equals(body, expected)) {
            throw new AssertionError("Expected body to equal '" + truncate(expected, 100) +
                    "' but was: " + truncate(body, 100));
        }
        return this;
    }

    /**
     * Return the underlying response for further custom assertions.
     */
    public HttpResponse<String> response() {
        return response;
    }

    private static String truncate(String s, int maxLen) {
        if (s == null) {
            return "<null>";
        }
        if (s.length() <= maxLen) {
            return s;
        }
        return s.substring(0, maxLen) + "...<truncated>";
    }
}
