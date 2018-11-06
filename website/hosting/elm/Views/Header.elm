module Views.Header exposing (view)

import Routing exposing (Route(..))
import Html exposing (..)
import Html.Attributes exposing (..)
import Html.Events exposing (..)
import Messages exposing (..)
import Svg
import Svg.Attributes
import Model exposing (..)


view : Model -> Html Msg
view model =
    header [ class "header" ] [
        div [ class "header-menu" ] [
            logo,
            headerMenu model
        ]
    ]

logoSvg : Html Msg
logoSvg =
    Svg.svg
        [ width 70
        , height 40
        , Svg.Attributes.viewBox "0 0 1060 460"
        , Svg.Attributes.version "1.1"
        ]
        [ Svg.defs []
            [ Svg.radialGradient
                  [ Svg.Attributes.cx "10.5172853%"
                  , Svg.Attributes.cy "100%"
                  , Svg.Attributes.fx "10.5172853%"
                  , Svg.Attributes.fy "100%"
                  , Svg.Attributes.r "119.945282%"
                  , Svg.Attributes.gradientTransform
                        """translate(0.105173,1.000000)
                          ,scale(1.000000,0.952381)
                          ,rotate(-51.739535)
                          ,scale(1.000000,0.892357)
                          ,translate(-0.105173,-1.000000)"""
                  , Svg.Attributes.id "radialGradient-1"
                  ]
                  [ Svg.stop
                        [ Svg.Attributes.stopColor "#3023AE"
                        , Svg.Attributes.offset "0%"
                        ] []
                  , Svg.stop
                        [ Svg.Attributes.stopColor "#53A0FD"
                        , Svg.Attributes.offset "79.8743881%"
                        ] []
                  , Svg.stop
                        [ Svg.Attributes.stopColor "#51DEEC"
                        , Svg.Attributes.offset "100%"
                        ] []
                  ]
            ]
        , Svg.g
              [ Svg.Attributes.id "logo"
              , Svg.Attributes.stroke "none"
              , Svg.Attributes.strokeWidth "1"
              , Svg.Attributes.fill "none"
              , Svg.Attributes.fillRule "evenodd" ]
              [ Svg.g
                    [ Svg.Attributes.id "Group" ]
                    [ Svg.g
                          [ Svg.Attributes.transform "translate(40.000000, 20.000000)" ]
                          [ Svg.path
                                [ Svg.Attributes.d
                                      """M0,200 C0,0 183.916355,
                                         3.55271368e-15 200,0 C216.083645,0 400,0 400,200 C400,
                                         360.602127 263.263237,400 200,400 C169.940075,
                                         400 130.053436,383.872837 110.053436,360 C83.687159,
                                         328.528108 81.6540229,287.441333 100,280 C120,
                                         271.887777 144.702055,
                                         320 200,320 C255.297945,320 320,270.651856 320,200 C320,
                                         129.348144 280,80 200,80 C120,80 79.9607444,129.348144 79.9607444,
                                         200 L79.9607444,380 C79.9607444,400.484491 52.5173973,
                                         420 40,420 C27.4826027,420 7.10542736e-15,398.833096 0,
                                         380 L0,200 Z M200,260 C166.862915,
                                         260 140,233.137085 140,200 C140,166.862915 166.862915,140 200,
                                         140 C233.137085,140 260,166.862915 260,200 C260,
                                         233.137085 233.137085,260 200,260 Z"""
                                , Svg.Attributes.id "Shape"
                                , Svg.Attributes.fill "url(#radialGradient-1)"
                                ] []
                          , Svg.text_
                                [ Svg.Attributes.id "poac"
                                , Svg.Attributes.fontFamily "VarelaRound, Varela Round"
                                , Svg.Attributes.fontSize "230"
                                , Svg.Attributes.fontWeight "normal"
                                , Svg.Attributes.letterSpacing "-12.5500002"
                                , Svg.Attributes.fill "#000000"
                                ]
                                [ Svg.tspan
                                      [ Svg.Attributes.x "423"
                                      , Svg.Attributes.y "367"
                                      ]
                                      [ text "poac" ]
                                ]
                          ]
                    ]
              ]
        ]

logo : Html Msg
logo =
    div [ class "header-logo" ]
        [ a [ onClick <| NavigateTo HomeIndexRoute
            , class "header-item header-item-logo"
            ] [ logoSvg ]
        ]


headerMenu : Model -> Html Msg
headerMenu model =
    let
        appendListItem =
            List.append
            [ menuItemPackages
            , menuItemDonate
            , menuItemDocs
            ]
        listItem =
            appendListItem <| signupOrUserInfo model
        lists =
            List.map toLi listItem
    in
        nav []
        [ ul [ class "header-list-menu" ] lists
        ]


toLi : Html Msg -> Html Msg
toLi item =
    li [] [ item ]



menuItemPackages : Html Msg
menuItemPackages =
    a [ onClick <| NavigateTo (PackagesRoute "")
        , class "header-item"
      ] [ text "PACKAGES" ]


menuItemDonate : Html Msg
menuItemDonate =
    a [ onClick <| NavigateTo DonateRoute
        , class "header-item"
        ] [ text "DONATE" ]


menuItemDocs : Html Msg
menuItemDocs =
    a [ href "https://docs.poac.pm/"
        , class "header-item"
        ] [ text "DOCS" ]


signupOrUserInfo : Model -> List (Html Msg)
signupOrUserInfo model =
    case model.signinUser of
        Success user ->
            [ userInfo user model.signinId ]
        Requesting ->
            [ text "Loading..." ]
        _ ->
            [ signin
            , signup
            ]


userInfo : SigninUser -> String -> Html Msg
userInfo user signinId =
    div [ class "dropdown" ]
    [ button [ class "dropbtn" ]
      [ img [ class "avatar"
            , alt signinId
            , src user.photo_url
            , width 20
            , height 20
            ] []
      , text user.name
      , span [ class "dropdown-caret" ] []
      ]
    , div [ class "dropdown-content" ]
        [ a [ onClick <| NavigateTo (UsersRoute signinId)
            , style [("cursor", "pointer"), ("color", "black")]
            ]
            [ text "Your Profile"
            ]
        , a [ onClick <| NavigateTo SettingRoute ]
            [ text "Settings"
            ]
        , hr [ class "dropdown-divider" ] []
        , a [ onClick <| Signout ]
            [ text "Sign out"
            ]
        ]
    ]

signin : Html Msg
signin =
    a [ class "sign in pulse"
        , onClick <| LoginOrSignup
      ] [ text "SIGNIN" ]

signup : Html Msg
signup =
    a [ class "sign up pulse"
        , onClick <| LoginOrSignup
      ] [ text "SIGNUP" ]
