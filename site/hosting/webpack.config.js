const path = require('path');
const MiniCssExtractPlugin = require('mini-css-extract-plugin');
const UglifyJsPlugin = require('uglifyjs-webpack-plugin');
const OptimizeCSSAssetsPlugin = require('optimize-css-assets-webpack-plugin');
const CopyWebpackPlugin = require('copy-webpack-plugin');

module.exports = {
    optimization: {
        minimizer: [
            new UglifyJsPlugin({
                cache: true,
                parallel: true,
                sourceMap: true
            }),
            new OptimizeCSSAssetsPlugin({})
        ]
    },

    entry: './js/app.js',

    output: {
        filename: 'app.js',
        path: path.resolve(__dirname, '../dist/js')
    },

    plugins: [
        new MiniCssExtractPlugin({
            filename: '../css/app.css'
        }),
        new CopyWebpackPlugin([{
            from: 'assets/',
            to: '../'
        }]),
        new CopyWebpackPlugin([{
            from: 'index.html',
            to: '../'
        }])
    ],

    module: {
        rules: [
            {
                test: /\.js$/,
                exclude: /node_modules/,
                use: {
                    loader: 'babel-loader'
                }
            },
            {
                test: /\.scss$/,
                use: [
                    MiniCssExtractPlugin.loader,
                    {
                        loader: "css-loader",
                        options: {
                            url: false,
                            minimize: true,
                            localIdentName: '[local]'
                        }
                    },
                    "sass-loader"
                ]
            },
            {
                test: /\.elm$/,
                exclude: [/elm-stuff/, /node_modules/],
                loader: "elm-webpack-loader",
                options: {
                    optimize: true
                }
            },
        ],
        noParse: [/\.elm$/]
    }
};
