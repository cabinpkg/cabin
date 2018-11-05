module Views.Footer exposing (view)

import Messages exposing (..)
import Html exposing (..)
import Html.Attributes exposing (..)


view : Html Msg
view =
    footer []
      [ hr [ class "border-hr" ] []
      , div [ class "footer-content" ]
          [ div [ class "links" ]
--              [ a [ href ("/" ++ (String.toLower "Policies")) ]
--                  [ text "Policies"
--                  ]
--              , a [ href ("/" ++ (String.toLower "Sponsors")) ]
--                  [ text "Sponsors"
--                  ]
                [ a [ href "mailto:support@poac.pm?subject=[Feedback]" ]
                    [ text "Feedback" ]
                , a [ href "https://github.com/poacpm" ]
                    [ text "GitHub" ]
                ]
          , div [ class "copyright" ]
                [ text "©︎ 2018 Ken Matsui" ]
          ]
      ]
