defmodule PoacpmWeb.PageControllerTest do
  use PoacpmWeb.ConnCase

  test "GET /", %{conn: conn} do
    conn = get(conn, "/")
    assert html_response(conn, 200) =~ "<meta name=\"author\" content=\"matken\">"
  end
end
